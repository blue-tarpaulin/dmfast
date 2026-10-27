//! Every write to BYOND's memory and the code each write relies on, by feature. A feature's writes are held
//! until its install returns `Ok`, then checked against earlier features' writes and relied-on ranges: on a
//! conflict none are done. Features read code as shipped (`crate::snapshot`), so the result doesn't depend on
//! install order.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use crate::feature::Feature;
use crate::sync::lock;
use crate::{Result, bail};

/// Does one write; `Err` if the memory can't be made writable.
pub(crate) type Poke = unsafe fn(usize, &[u8]) -> Result<()>;

struct Write {
    addr: usize,
    data: Vec<u8>,
    poke: Poke,
    wrap: bool,
}

/// Runs once the installing feature's writes are decided ([`on_commit`], [`on_abort`]).
pub(crate) type Action = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Pending {
    writes: Vec<Write>,
    relied: Vec<(usize, usize)>,
    /// Relied on through a shared lookup.
    shared: Vec<SharedRange>,
    /// Run in order after the writes.
    commit: Vec<Action>,
    /// Run newest first if the writes aren't done.
    abort: Vec<Action>,
}

/// A range relied on through a [`Ctx::shared`] lookup: (start, end, its key).
///
/// [`Ctx::shared`]: crate::feature::Ctx::shared
pub(crate) type SharedRange = (usize, usize, &'static str);

/// How much the installing feature had recorded; [`rollback`] drops what came after.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    writes: usize,
    relied: usize,
    shared: usize,
    commit: usize,
    abort: usize,
}

/// The installing feature's ranges; `None` outside an install.
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// From [`begin`] to the end of [`commit`] or [`discard`]: lets [`rely`] skip the lock outside an install.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// A range of an installed feature: (start, end, by).
type Range = (usize, usize, &'static Feature);

static WRITTEN: Mutex<Vec<Range>> = Mutex::new(Vec::new());

static RELIED: Mutex<Vec<Range>> = Mutex::new(Vec::new());

/// Ranges installed features relied on through a shared lookup: (start, end, by, key).
static SHARED: Mutex<Vec<(usize, usize, &'static Feature, &'static str)>> = Mutex::new(Vec::new());

/// Record a write of `data` at `addr` with `poke` by the installing feature, done if it installs; `Err` outside an
/// install. `wrap` may overlap features in `after`.
pub(crate) fn record(addr: usize, data: &[u8], poke: Poke, wrap: bool) -> Result<()> {
    match lock(&PENDING).as_mut() {
        Some(p) => {
            p.writes.push(Write { addr, data: data.to_vec(), poke, wrap });
            Ok(())
        }
        None => Err(format!("write at {addr:#x} outside an install").into()),
    }
}

/// A feature is installing: no other thread runs BYOND's code.
pub(crate) fn installing() -> bool {
    INSTALLING.load(Relaxed)
}

/// The installing feature relies on `addr..addr+len` (see the module docs); nothing outside an install.
#[inline]
pub(crate) fn rely(addr: usize, len: usize) {
    if len != 0 && INSTALLING.load(Relaxed) {
        rely_slow(addr, len);
    }
}

#[cold]
fn rely_slow(addr: usize, len: usize) {
    if let Some(p) = lock(&PENDING).as_mut() {
        p.relied.push((addr, addr.saturating_add(len)));
    }
}

pub(super) fn begin() {
    *lock(&PENDING) = Some(Pending::default());
    INSTALLING.store(true, Relaxed);
}

/// `f` runs after the installing feature's writes are done; dropped outside an install.
pub(crate) fn on_commit(f: Action) {
    if let Some(p) = lock(&PENDING).as_mut() {
        p.commit.push(f);
    }
}

/// `f` runs if the installing feature's writes (or its part's) aren't done; dropped outside an install.
pub(crate) fn on_abort(f: Action) {
    if let Some(p) = lock(&PENDING).as_mut() {
        p.abort.push(f);
    }
}

/// What the installing feature has recorded so far; `None` outside an install.
pub(crate) fn mark() -> Option<Mark> {
    lock(&PENDING).as_ref().map(|p| Mark {
        writes: p.writes.len(),
        relied: p.relied.len(),
        shared: p.shared.len(),
        commit: p.commit.len(),
        abort: p.abort.len(),
    })
}

/// Drop what the installing feature recorded after `m`, running the dropped [`on_abort`] actions.
pub(crate) fn rollback(m: Mark) {
    let aborts = match lock(&PENDING).as_mut() {
        Some(p) => {
            p.writes.truncate(m.writes);
            p.relied.truncate(m.relied);
            p.shared.truncate(m.shared);
            p.commit.truncate(m.commit);
            p.abort.split_off(m.abort.min(p.abort.len()))
        }
        None => Vec::new(),
    };
    run_aborts(aborts);
}

/// What the installing feature relied on after `m` is relied on through the shared lookup `key` (ranges a nested
/// lookup relied on keep its key); returns all of it, for later users ([`replay`]).
pub(crate) fn share(m: Mark, key: &'static str) -> Vec<SharedRange> {
    let mut guard = lock(&PENDING);
    let Some(p) = guard.as_mut() else { return Vec::new() };
    let own = merged(p.relied.split_off(m.relied.min(p.relied.len())));
    p.shared.extend(own.into_iter().map(|(start, end)| (start, end, key)));
    p.shared.get(m.shared..).unwrap_or_default().to_vec()
}

/// The installing feature relies on `ranges` through the shared lookups they came from ([`share`]); nothing outside
/// an install.
pub(crate) fn replay(ranges: &[SharedRange]) {
    if let Some(p) = lock(&PENDING).as_mut() {
        p.shared.extend_from_slice(ranges);
    }
}

fn run_aborts(aborts: Vec<Action>) {
    aborts.into_iter().rev().for_each(|f| f());
}

/// Its install function failed: drop its ranges and run its [`on_abort`] actions.
pub(super) fn discard() {
    let pending = lock(&PENDING).take();
    INSTALLING.store(false, Relaxed);
    if let Some(p) = pending {
        run_aborts(p.abort);
    }
}

/// Its install succeeded: do its writes unless one conflicts with another feature's (see the module docs), then
/// its [`on_commit`] actions; on a conflict its [`on_abort`] actions. If a write fails, the ones before it stay
/// done, so the actions their hooks need run too (or, with none done, the abort actions).
///
/// # Safety
/// No other thread runs the code written (install time).
pub(super) unsafe fn commit(f: &'static Feature) -> Result<()> {
    let pending = lock(&PENDING).take();
    let result = match pending {
        Some(p) => unsafe { apply(f, p) },
        None => Ok(()),
    };
    INSTALLING.store(false, Relaxed);
    result
}

/// [`commit`]'s work; actions run while [`installing`] still holds, so they may set install-time state.
unsafe fn apply(f: &'static Feature, p: Pending) -> Result<()> {
    let Pending { writes, relied, shared, commit, abort } = p;
    let relied = merged(relied);
    let spans: Vec<(usize, usize, bool)> = writes.iter().map(|w| (w.addr, w.addr + w.data.len(), w.wrap)).collect();
    let clash = conflict(f, &spans, &relied, &shared, &lock(&WRITTEN), &lock(&RELIED), &lock(&SHARED));
    if let Some((by, at)) = clash {
        run_aborts(abort);
        bail!("conflicts with {by} at {at:#x}");
    }
    for (i, w) in writes.into_iter().enumerate() {
        if let Err(e) = unsafe { (w.poke)(w.addr, &w.data) } {
            if i == 0 {
                run_aborts(abort);
            } else {
                commit.into_iter().for_each(|a| a());
            }
            return Err(e);
        }
        lock(&WRITTEN).push((w.addr, w.addr + w.data.len(), f));
    }
    lock(&RELIED).extend(relied.into_iter().map(|(start, end)| (start, end, f)));
    lock(&SHARED).extend(shared.into_iter().map(|(start, end, key)| (start, end, f, key)));
    commit.into_iter().for_each(|a| a());
    Ok(())
}

/// `ranges` sorted, overlapping and touching ones joined.
fn merged(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

/// First conflict of `f`'s writes (start, end, wrap), relied ranges and ranges relied on through shared lookups
/// with earlier ones: (other's key, address). Users of one shared lookup may write over what it relied on.
fn conflict(
    f: &Feature,
    writes: &[(usize, usize, bool)],
    relied: &[(usize, usize)],
    shared: &[SharedRange],
    written: &[Range],
    read: &[Range],
    read_shared: &[(usize, usize, &'static Feature, &'static str)],
) -> Option<(&'static str, usize)> {
    let overlap = |(a, b): (usize, usize), (c, d): (usize, usize)| (a < d && c < b).then(|| a.max(c));
    let wraps = |by: &Feature| f.after.iter().any(|a| std::ptr::eq(*a, by));
    // A wrap write over `by`'s range, `by` in `after`.
    let wrapped =
        |(s, e, by): Range| wraps(by) && writes.iter().any(|&(a, b, w)| w && overlap((a, b), (s, e)).is_some());
    let uses = |key: &str| shared.iter().any(|r| r.2 == key);
    let used = |by: &Feature, key: &str| read_shared.iter().any(|r| std::ptr::eq(r.2, by) && r.3 == key);
    for &(a, b, wrap) in writes {
        for &(s, e, by) in written.iter().chain(read) {
            if let Some(at) = overlap((a, b), (s, e))
                && !(wrap && wraps(by))
            {
                return Some((by.key, at));
            }
        }
    }
    for &r in relied {
        for &(s, e, by) in written {
            if let Some(at) = overlap(r, (s, e))
                && !wrapped((s, e, by))
            {
                return Some((by.key, at));
            }
        }
    }
    for &(a, b, wrap) in writes {
        for &(s, e, by, key) in read_shared {
            if let Some(at) = overlap((a, b), (s, e))
                && !(wrap && wraps(by))
                && !uses(key)
            {
                return Some((by.key, at));
            }
        }
    }
    for &(a, b, key) in shared {
        for &(s, e, by) in written {
            if let Some(at) = overlap((a, b), (s, e))
                && !wrapped((s, e, by))
                && !used(by, key)
            {
                return Some((by.key, at));
            }
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::feature::{Ctx, Hosts, Install};
    use crate::image::Image;
    use std::cell::Cell;

    const NONE: Install = Install { os: 1, run: None };
    static A: Feature = Feature::perf(Hosts::SERVER, "a", "", NONE);
    static B: Feature = Feature::perf(Hosts::SERVER, "b", "", NONE);
    static WRAPS_A: Feature = Feature::perf(Hosts::SERVER, "w", "", NONE).after(&[&A]);

    /// `f`'s conflict with an earlier feature that wrote `written` and relied on `read` (each as `by`).
    fn check(
        f: &Feature,
        writes: &[(usize, usize, bool)],
        relied: &[(usize, usize)],
        by: &'static Feature,
        written: &[(usize, usize)],
        read: &[(usize, usize)],
    ) -> Option<(&'static str, usize)> {
        let tag = |r: &[(usize, usize)]| r.iter().map(|&(s, e)| (s, e, by)).collect::<Vec<_>>();
        conflict(f, writes, relied, &[], &tag(written), &tag(read), &[])
    }

    /// Users of one shared lookup may write over what it relied on; other features' writes there conflict, and so
    /// does relying on those bytes another way.
    #[test]
    fn shared_lookup_users_may_write_over_its_ranges() {
        let read_shared: [(usize, usize, &'static Feature, &str); 1] = [(100, 140, &A, "k")];
        let write = [(109, 133, false)];
        assert_eq!(conflict(&B, &write, &[], &[(100, 140, "k")], &[], &[], &read_shared), None);
        assert_eq!(conflict(&B, &write, &[], &[(100, 140, "j")], &[], &[], &read_shared), Some(("a", 109)));
        // A, a user of "k", wrote 109..133.
        let written = [(109, 133, &A)];
        assert_eq!(conflict(&B, &[], &[], &[(100, 140, "k")], &written, &[], &read_shared), None);
        assert_eq!(conflict(&B, &[], &[], &[(100, 140, "j")], &written, &[], &read_shared), Some(("a", 109)));
        assert_eq!(conflict(&B, &[], &[(100, 140)], &[], &written, &[], &read_shared), Some(("a", 109)));
    }

    #[test]
    fn writes_conflict_with_writes() {
        assert_eq!(check(&B, &[(10, 15, false)], &[], &A, &[(14, 20)], &[]), Some(("a", 14)));
        assert_eq!(check(&B, &[(10, 15, false)], &[], &A, &[(15, 20)], &[]), None);
    }

    /// A signature (or copied instructions) over another feature's write conflicts, in either install order.
    #[test]
    fn reads_conflict_with_writes_both_ways() {
        // B relies on 100..140; A wrote 109..133 first.
        assert_eq!(check(&B, &[(500, 505, false)], &[(100, 140)], &A, &[(109, 133)], &[]), Some(("a", 109)));
        // A relied on 100..140 first; B writes 109..133.
        assert_eq!(check(&B, &[(109, 133, false)], &[], &A, &[], &[(100, 140)]), Some(("a", 109)));
        // Reads never conflict with reads, nor with ranges next to them.
        assert_eq!(check(&B, &[(140, 145, false)], &[(100, 140)], &A, &[(90, 100)], &[(100, 140)]), None);
    }

    #[test]
    fn a_wrap_may_build_on_the_wrapped_hook() {
        // A hooked 200..206 relying on 199..206; the wrapper reads A's hook site and wraps it.
        let (written, read) = (&[(200, 206)][..], &[(199, 206)][..]);
        assert_eq!(check(&WRAPS_A, &[(200, 205, true)], &[(199, 205)], &A, written, read), None);
        // Without `wrap_code`, or over a feature not in `after`, it is a conflict.
        assert_eq!(check(&WRAPS_A, &[(200, 205, false)], &[], &A, written, read), Some(("a", 200)));
        assert_eq!(check(&WRAPS_A, &[(200, 205, true)], &[], &B, written, read), Some(("b", 200)));
        // Relying on another of A's writes is still a conflict.
        assert_eq!(
            check(&WRAPS_A, &[(200, 205, true)], &[(300, 304)], &A, &[(200, 206), (302, 310)], read),
            Some(("a", 302))
        );
    }

    #[test]
    fn merges_ranges() {
        assert_eq!(merged(vec![(5, 9), (0, 2), (2, 4), (8, 12), (20, 21)]), [(0, 4), (5, 12), (20, 21)]);
    }

    /// The ledger (and the snapshot) are global: tests that install or read images run one at a time, each over
    /// its own addresses.
    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

    /// What the installing feature relies on so far.
    pub(crate) fn relied_now() -> Vec<(usize, usize)> {
        lock(&PENDING).as_ref().map(|p| p.relied.clone()).unwrap_or_default()
    }

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn log(s: String) {
        lock(&LOG).push(s);
    }

    fn take_log() -> Vec<String> {
        std::mem::take(&mut *lock(&LOG))
    }

    unsafe fn poke(addr: usize, _: &[u8]) -> Result<()> {
        log(format!("poke {addr:#x}"));
        Ok(())
    }

    unsafe fn failing(addr: usize, _: &[u8]) -> Result<()> {
        Err(format!("cannot write {addr:#x}").into())
    }

    fn write(addr: usize) {
        record(addr, &[0; 4], poke, false).unwrap();
    }

    fn actions(name: &'static str) {
        on_commit(Box::new(move || log(format!("commit {name}"))));
        on_abort(Box::new(move || log(format!("abort {name}"))));
    }

    fn written_by(f: &'static Feature, from: usize, to: usize) -> Vec<(usize, usize)> {
        let w = lock(&WRITTEN);
        w.iter().filter(|r| std::ptr::eq(r.2, f) && r.0 >= from && r.1 <= to).map(|r| (r.0, r.1)).collect()
    }

    #[test]
    fn rollback_drops_a_parts_records_and_runs_its_aborts() {
        let _serial = lock(&SERIAL);
        take_log();
        begin();
        write(0x1000);
        rely(0x1100, 4);
        actions("1");
        let m = mark().unwrap();
        write(0x1010);
        rely(0x1110, 4);
        actions("2");
        actions("3");
        rollback(m);
        assert_eq!(take_log(), ["abort 3", "abort 2"]);
        assert!(unsafe { commit(&A) }.is_ok());
        assert_eq!(take_log(), ["poke 0x1000", "commit 1"]);
        assert_eq!(written_by(&A, 0x1000, 0x2000), [(0x1000, 0x1004)]);
        let relied: Vec<_> = lock(&RELIED).iter().filter(|r| (0x1000..0x2000).contains(&r.0)).map(|r| r.0).collect();
        assert_eq!(relied, [0x1100]);
    }

    #[test]
    fn commit_runs_actions_after_the_writes_in_order() {
        let _serial = lock(&SERIAL);
        take_log();
        begin();
        actions("1");
        write(0x2000);
        actions("2");
        write(0x2010);
        assert!(unsafe { commit(&A) }.is_ok());
        assert_eq!(take_log(), ["poke 0x2000", "poke 0x2010", "commit 1", "commit 2"]);
        assert!(!installing());
    }

    #[test]
    fn discard_and_conflict_run_aborts_newest_first() {
        let _serial = lock(&SERIAL);
        take_log();
        begin();
        write(0x3000);
        actions("1");
        actions("2");
        discard();
        assert_eq!(take_log(), ["abort 2", "abort 1"]);
        assert!(!installing());

        begin();
        write(0x3000);
        assert!(unsafe { commit(&A) }.is_ok());
        take_log();
        begin();
        actions("1");
        write(0x3002);
        actions("2");
        assert_eq!(unsafe { commit(&B) }, Err("conflicts with a at 0x3002".into()));
        assert_eq!(take_log(), ["abort 2", "abort 1"]);
        assert!(written_by(&B, 0x3000, 0x4000).is_empty());
    }

    #[test]
    fn a_failed_write_runs_aborts_only_before_any_write() {
        let _serial = lock(&SERIAL);
        take_log();
        begin();
        record(0x4000, &[0; 4], failing, false).unwrap();
        write(0x4010);
        actions("1");
        assert_eq!(unsafe { commit(&A) }, Err("cannot write 0x4000".into()));
        assert_eq!(take_log(), ["abort 1"]);
        // A write is done: its hook may need the state the commit actions publish.
        begin();
        write(0x4020);
        record(0x4030, &[0; 4], failing, false).unwrap();
        actions("2");
        assert_eq!(unsafe { commit(&A) }, Err("cannot write 0x4030".into()));
        assert_eq!(take_log(), ["poke 0x4020", "commit 2"]);
    }

    #[test]
    fn actions_outside_an_install_are_dropped() {
        let _serial = lock(&SERIAL);
        take_log();
        actions("1");
        assert!(mark().is_none());
        begin();
        assert!(unsafe { commit(&A) }.is_ok());
        assert!(take_log().is_empty());
    }

    /// A `Ctx` over `code` (a test's own static: no other test relies on or writes its addresses).
    fn code_ctx(code: &'static [u8; 8]) -> (Ctx, usize) {
        let a = code.as_ptr() as usize;
        // SAFETY: `code` is static.
        let core = unsafe { Image::new(a, vec![(a, 8)]) }.with_parts(vec![(a, 8, true)]).unwrap();
        (Ctx::new(core, None), a)
    }

    /// A shared lookup reading `core.base + 2 .. + 6`, counting its runs.
    fn lookup<'a>(cx: &'a Ctx, runs: &Cell<u32>) -> crate::Result<&'a usize> {
        cx.shared("test", |cx| {
            runs.set(runs.get() + 1);
            let at = cx.core().base + 2;
            cx.core().bytes(at, 4)?;
            Ok(at)
        })
    }

    /// What the installing feature relied on through shared lookups after `m`.
    fn shared_since(m: Mark) -> Vec<SharedRange> {
        lock(&PENDING).as_ref().map(|p| p.shared[m.shared..].to_vec()).unwrap_or_default()
    }

    /// It runs once; later users rely on what it read even though the first user failed. A user may write over
    /// those bytes without a conflict with the others; any other feature's write there conflicts.
    #[test]
    fn shared_lookups_replay_what_they_relied_on() {
        static C: Feature = Feature::perf(Hosts::SERVER, "c", "", NONE);
        let _serial = lock(&SERIAL);
        static CODE: [u8; 8] = [0x90; 8];
        let ((cx, a), runs) = (code_ctx(&CODE), Cell::new(0));
        begin();
        assert_eq!(lookup(&cx, &runs), Ok(&(a + 2)));
        discard();
        begin();
        let m = mark().unwrap();
        assert_eq!(lookup(&cx, &runs), Ok(&(a + 2)));
        assert_eq!((runs.get(), shared_since(m)), (1, vec![(a + 2, a + 6, "test")]));
        record(a + 4, &[0; 2], poke, false).unwrap();
        assert!(unsafe { commit(&A) }.is_ok());
        begin();
        assert_eq!(lookup(&cx, &runs), Ok(&(a + 2)));
        assert!(unsafe { commit(&B) }.is_ok());
        begin();
        record(a + 3, &[0; 1], poke, false).unwrap();
        assert_eq!(unsafe { commit(&C) }, Err(format!("conflicts with a at {:#x}", a + 3).into()));
        assert_eq!(runs.get(), 1);
    }

    /// A failed lookup is kept too (replayed with its error); outside an install it runs on every call, and those
    /// runs aren't kept for installs.
    #[test]
    fn shared_lookup_errors_and_calls_outside_an_install() {
        let _serial = lock(&SERIAL);
        static CODE: [u8; 8] = [0xCC; 8];
        let ((cx, a), runs) = (code_ctx(&CODE), Cell::new(0));
        let failing = |cx: &Ctx| {
            let r = cx.shared::<()>("failing", |cx| {
                runs.set(runs.get() + 1);
                cx.core().bytes(a, 1)?;
                Err("not found".into())
            });
            r.map(|&()| ())
        };
        begin();
        assert_eq!(failing(&cx), Err("not found".into()));
        discard();
        begin();
        let m = mark().unwrap();
        let r = (failing(&cx), runs.get(), shared_since(m));
        assert_eq!(r, (Err("not found".into()), 1, vec![(a, a + 1, "failing")]));
        discard();

        let runs = Cell::new(0);
        assert_eq!((lookup(&cx, &runs), lookup(&cx, &runs)), (Ok(&(a + 2)), Ok(&(a + 2))));
        begin();
        assert_eq!((lookup(&cx, &runs), lookup(&cx, &runs)), (Ok(&(a + 2)), Ok(&(a + 2))));
        discard();
        assert_eq!(runs.get(), 3);
    }
}
