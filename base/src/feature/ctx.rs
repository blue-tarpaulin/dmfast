//! [`Ctx`], what an install function gets, and [`Parts`], a feature's independently installed parts.

use std::any::Any;
use std::cell::OnceCell;
use std::marker::PhantomData;

use crate::image::Image;
use crate::{Error, Result};

/// What an install function gets: the images it reads and writes, and the lookups features share
/// ([`Ctx::shared`]), only while features install ([`crate::image::Image`]).
pub struct Ctx {
    core: Image,
    /// `None`: the loader didn't report it (Linux) or its headers didn't parse (Windows).
    exe: Option<Image>,
    shared: Memo,
}

impl Ctx {
    pub(crate) fn new(core: Image, exe: Option<Image>) -> Ctx {
        Ctx { core, exe, shared: Memo::default() }
    }

    /// The core library (byondcore.dll, libbyond.so).
    pub fn core(&self) -> &Image {
        &self.core
    }

    /// The program; `Err` if the loader left it out of its list (Linux) or its headers didn't parse (Windows).
    pub fn exe(&self) -> Result<&Image> {
        self.exe.as_ref().ok_or_else(|| "program image not reported by the loader".into())
    }

    /// Another loaded module (e.g. `byondext.dll`, `libc.so.6`). Its code is copied when first opened, so its
    /// readers see it as shipped too (`crate::snapshot`).
    pub fn module(&self, name: &str) -> Result<Image> {
        Image::loaded(name).inspect(crate::snapshot::take)
    }

    /// `f(self)`, run once for `key` (a lookup several features start from) and kept until install ends. Each
    /// caller relies on what `f` relied on, as if it had run `f` itself, so another feature's write over those
    /// bytes conflicts even if the first caller didn't install; the callers themselves may write there (they share
    /// its sites). `f` only reads: what else it recorded (writes, actions) is the first caller's. Outside an install
    /// (e.g. in [`Feature::offered_if`]) `f` runs on every call.
    pub fn shared<T: 'static>(&self, key: &'static str, f: impl FnOnce(&Ctx) -> Result<T>) -> Result<&T> {
        let kept = self.shared.iter().find(|s| s.key == key && s.relied.is_some());
        let s = match kept {
            Some(s) => {
                crate::ledger::replay(s.relied.as_deref().unwrap_or_default());
                s
            }
            None => {
                let mark = crate::ledger::mark();
                let value: Box<dyn Any> = Box::new(f(self));
                let relied = mark.map(|m| crate::ledger::share(m, key));
                self.shared.push(Shared { key, value, relied })
            }
        };
        let value = s.value.downcast_ref::<Result<T>>();
        value
            .ok_or_else(|| Error::from(format!("shared lookup {key} kept as another type")))?
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// [`Ctx::shared`]'s lookups: an append-only list, so what it hands out stays put while the `Ctx` lives.
#[derive(Default)]
struct Memo {
    entry: OnceCell<Shared>,
    next: OnceCell<Box<Memo>>,
}

struct Shared {
    key: &'static str,
    /// The lookup's `Result<T>`.
    value: Box<dyn Any>,
    /// The ranges it relied on, replayed into each later caller's; `None`: it ran outside an install (not kept).
    relied: Option<Vec<crate::ledger::SharedRange>>,
}

impl Memo {
    fn iter(&self) -> impl Iterator<Item = &Shared> {
        std::iter::successors(Some(self), |m| m.next.get().map(|b| &**b)).filter_map(|m| m.entry.get())
    }

    /// `s`, added at the end.
    fn push(&self, s: Shared) -> &Shared {
        let mut m = self;
        while m.entry.get().is_some() {
            m = m.next.get_or_init(Box::default);
        }
        m.entry.get_or_init(|| s)
    }
}

impl Ctx {
    /// Run `f` once this feature's writes are done (not after `Err` or a conflict), still at install time, in
    /// order. Publish state other features read here, so a feature that doesn't install leaves none.
    pub fn on_commit(&self, f: impl FnOnce() + Send + 'static) {
        crate::ledger::on_commit(Box::new(f));
    }

    /// Independently installed parts of this feature.
    pub fn parts(&self) -> Parts<'_> {
        Parts { _cx: PhantomData, done: Vec::new(), missing: Vec::new() }
    }
}

/// Parts of a feature that install independently: a part that fails is left out with everything it recorded
/// (writes, relied ranges, [`Ctx::on_commit`] actions, its stubs freed), the others still apply.
pub struct Parts<'a> {
    _cx: PhantomData<&'a Ctx>,
    done: Vec<String>,
    missing: Vec<Error>,
}

impl Parts<'_> {
    /// Install a part: `Ok(detail)` lists `detail`, `Err(why)` undoes it and lists `why`. Whether it installed.
    pub fn part(&mut self, f: impl FnOnce() -> Result<String>) -> bool {
        let mark = crate::ledger::mark();
        match f() {
            Ok(detail) => {
                self.done.push(detail);
                true
            }
            Err(why) => {
                if let Some(m) = mark {
                    crate::ledger::rollback(m);
                }
                self.missing.push(why);
                false
            }
        }
    }

    /// [`Parts::part`] listed as `name`, or `name (why)`.
    pub fn named(&mut self, name: &str, f: impl FnOnce() -> Result<()>) -> bool {
        self.part(|| f().map(|()| name.to_string()).map_err(|e| format!("{name} ({e})").into()))
    }

    /// List a part left out without [`Parts::part`] (what was read to decide stays relied on).
    pub fn skip(&mut self, why: impl Into<Error>) {
        self.missing.push(why.into());
    }

    /// Whether some part installed.
    pub fn any(&self) -> bool {
        !self.done.is_empty()
    }

    /// `Err` only if no part installed, else the parts and why the others are missing.
    pub fn finish(self) -> Result<String> {
        let missing = self.missing.iter().map(Error::as_str).collect::<Vec<_>>().join("; ");
        if self.done.is_empty() {
            return Err(missing.into());
        }
        if self.missing.is_empty() {
            Ok(self.done.join(", "))
        } else {
            Ok(format!("{}; without: {missing}", self.done.join(", ")))
        }
    }
}
