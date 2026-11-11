//! DreamSeeker's patches (`dmfast_seeker`), written on disk into dreamseeker.exe, byondcore.dll, and byondwin.dll,
//! with the stock files kept in `bin/backup/`. dmfast.dll itself never goes into dreamseeker.exe. A build without
//! private patches carries none, and its install undoes an earlier install's.
//!
//! Install and uninstall first work out a [`Plan`] (reading, applying, and checking the files aren't in use), so a
//! failure stops them before anything is written, then [`carry_out`] writes it.

use std::fs;
use std::path::{Path, PathBuf};

use dmfast_format::fingerprint::fnv1a;
use dmfast_format::manifest::CODE_SECTION;
use dmfast_seeker::patch::{Module, Patch};
use dmfast_seeker::{apply, audit, author};

use super::{ensure_not_in_use, pe};
use crate::files::{Failure, io_err, read, rename_retry, write};
use crate::fixes;

/// The private patches: each folder's key, compiled object, and spec (none in a build without them).
const CLIENT: &[(&str, &[u8], &str)] = include!(concat!(env!("OUT_DIR"), "/client.rs"));
/// The program the patches run in. Every gate checks for its PE identity after patching.
const EXE: &str = "dreamseeker.exe";
/// The folder in `bin` holding the stock files (and nothing else).
const BACKUP: &str = "backup";

fn backup_of(dir: &Path, module: Module) -> PathBuf {
    dir.join(BACKUP).join(module.file())
}

fn is_patched(data: &[u8]) -> bool {
    pe::has_section(data, &CODE_SECTION)
}

/// `module`'s stock bytes (its backup) if it is patched, else `None`. Fails if it is patched and the backup is missing.
fn stock_of(dir: &Path, module: Module, live: &[u8]) -> Result<Option<Vec<u8>>, Failure> {
    if !is_patched(live) {
        return Ok(None);
    }
    let backup = backup_of(dir, module);
    if !backup.is_file() {
        return Err(format!(
            "{} has DreamSeeker patches but {} is missing; reinstall BYOND",
            module.file(),
            backup.display()
        )
        .into());
    }
    Ok(Some(read(&backup)?))
}

/// The first of `p`'s required files that is missing from `bin` or has a different hash.
fn changed_file<'a>(dir: &Path, p: &'a Patch) -> Option<&'a str> {
    let differs = |(name, hash): &&(String, u64)| read(&dir.join(name)).ok().map(|b| fnv1a(&b, &[])) != Some(*hash);
    p.files.iter().find(differs).map(|(name, _)| name.as_str())
}

/// What install or uninstall does to one module.
enum Step {
    /// Keep `stock` as the backup (when it isn't one yet), then write `image` as the module.
    Patch { module: Module, stock: Option<Vec<u8>>, image: Vec<u8> },
    /// Put the backup back as the module.
    Restore(Module),
    /// The module is stock: its backup, from before a BYOND update, goes.
    DropBackup(Module),
}

/// Everything install or uninstall does to the DLLs, checked before anything is written.
pub struct Plan {
    steps: Vec<Step>,
    /// For the message: what was applied, skipped, restored, or left.
    lines: Vec<String>,
    /// Install: DreamSeeker has patches afterwards.
    pub patched: bool,
}

impl Plan {
    /// Nothing to do or report: no DLL has DreamSeeker patches, gets them, or has a backup to remove.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.lines.is_empty()
    }

    /// What it will apply, skip, restore, or leave, for the message.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

/// The patches this build carries.
fn embedded() -> Result<Vec<Patch>, Failure> {
    let mut patches = Vec::new();
    for (key, obj, spec) in CLIENT {
        patches.extend(
            author::from_object(obj, spec, key).map_err(|e| format!("internal: DreamSeeker patch {key}: {e}"))?,
        );
    }
    Ok(patches)
}

pub fn has_client_patches() -> bool {
    !CLIENT.is_empty()
}

/// Whether any of DreamSeeker's modules has patches now.
pub fn client_patched(dir: &Path) -> bool {
    Module::ALL.iter().any(|m| read(&dir.join(m.file())).is_ok_and(|d| is_patched(&d)))
}

/// `--help client`: each patch's key, tier, and what it does.
pub fn client_list() -> String {
    let patches = match embedded() {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => return "This build carries no DreamSeeker patches.".into(),
        Err(e) => return e.message().into(),
    };
    let mut out = String::from(
        "DreamSeeker patches (details in CLIENT.md), applied with --client: the 'safe' ones, or those --fixes picks\n\
         like bug fixes. key, tier, what it does:\n",
    );
    let mut seen = Vec::new();
    for p in &patches {
        if !seen.contains(&&p.key) {
            seen.push(&p.key);
            out += &format!("  {:<16} {:<9} {}\n", p.key, p.tier.name(), p.about);
        }
    }
    out
}

/// Without a saved fix selection, `--client` takes the `safe` patches.
const CLIENT_DEFAULT: &str = "safe";

/// Install: with `client`, applies the patches the saved fix selection picks (or [`CLIENT_DEFAULT`]'s without one) to
/// each module's stock bytes (its backup if it is patched, else the file). dreamseeker.exe goes first, because the DLLs'
/// gates check for its identity after patching. A module that gets no patch (every module without `client`) goes back
/// to stock.
pub fn plan_install(dir: &Path, client: bool) -> Result<Plan, Failure> {
    let selection = if !client {
        Vec::new()
    } else {
        match fixes::current(dir)? {
            None => vec![CLIENT_DEFAULT.into()],
            Some(v) => fixes::entries(&v)?,
        }
    };
    let mut patches: Vec<Patch> = embedded()?.into_iter().filter(|p| p.wanted(&selection)).collect();
    let mut plan = Plan { steps: Vec::new(), lines: Vec::new(), patched: false };
    if client && patches.is_empty() {
        plan.lines.push("DreamSeeker: the fix selection picks none of its patches ('--help client' lists them)".into());
    }
    let exe_path = dir.join(EXE);
    let exe = if patches.is_empty() {
        None
    } else if exe_path.is_file() {
        let live = read(&exe_path)?;
        Some(stock_of(dir, Module::Seeker, &live)?.unwrap_or(live))
    } else {
        plan.lines.push(format!("DreamSeeker patches skipped: no {EXE} in {}", dir.display()));
        None
    };
    patches.retain(|p| match changed_file(dir, p) {
        None => true,
        Some(file) => {
            let why = format!("{file} isn't the one it was written for");
            plan.lines.push(format!("DreamSeeker, {}: {}: skipped ({why})", p.module.file(), p.name));
            false
        }
    });
    let mut exe_id = None;
    for module in Module::ALL {
        let path = dir.join(module.file());
        if !path.is_file() {
            continue;
        }
        let live = read(&path)?;
        let kept = stock_of(dir, module, &live)?;
        let patched = kept.is_some();
        let image = match &exe {
            Some(exe) => {
                let stock = kept.as_deref().unwrap_or(&live[..]);
                let id = match module {
                    Module::Seeker => None,
                    _ => Some(exe_id.ok_or("internal: dreamseeker.exe wasn't patched first")?),
                };
                let applied = apply::apply(stock, module, exe, id, &patches)
                    .map_err(|e| format!("DreamSeeker patches for {}: {e}", module.file()))?;
                plan.lines.extend(applied.lines.iter().map(|l| format!("DreamSeeker, {}: {l}", module.file())));
                if module == Module::Seeker {
                    exe_id = Some(audit::exe_id(applied.image.as_deref().unwrap_or(stock))?);
                }
                applied.image
            }
            None => None,
        };
        match image {
            Some(image) if image == live => plan.patched = true,
            Some(image) => {
                ensure_not_in_use(&path)?;
                plan.patched = true;
                plan.steps.push(Step::Patch { module, stock: (!patched).then_some(live), image });
            }
            None if patched => {
                ensure_not_in_use(&path)?;
                plan.steps.push(Step::Restore(module));
                plan.lines.push(format!("restored stock {}", module.file()));
            }
            None if backup_of(dir, module).is_file() => plan.steps.push(Step::DropBackup(module)),
            None => {}
        }
    }
    Ok(plan)
}

/// Uninstall: restores each patched module from its backup. A module whose backup is missing keeps its patches, with a
/// warning (a DLL's gates still run BYOND's code outside DreamSeeker).
pub fn plan_uninstall(dir: &Path) -> Result<Plan, Failure> {
    let mut plan = Plan { steps: Vec::new(), lines: Vec::new(), patched: false };
    for module in Module::ALL {
        let path = dir.join(module.file());
        if !path.is_file() {
            continue;
        }
        let backup = backup_of(dir, module);
        if !is_patched(&read(&path)?) {
            if backup.is_file() {
                plan.steps.push(Step::DropBackup(module));
            }
        } else if backup.is_file() {
            ensure_not_in_use(&path)?;
            plan.steps.push(Step::Restore(module));
            plan.lines.push(format!("restored stock {}", module.file()));
        } else {
            plan.lines.push(format!(
                "warning: {} was missing, so {} keeps its DreamSeeker patches; reinstall BYOND to get it stock",
                backup.display(),
                module.file()
            ));
        }
    }
    Ok(plan)
}

/// Writes `plan`, then removes `bin/backup/` if nothing is left in it. Returns the lines for the message.
pub fn carry_out(dir: &Path, plan: Plan) -> Result<Vec<String>, Failure> {
    let folder = dir.join(BACKUP);
    let Plan { steps, lines, .. } = plan;
    for step in steps {
        match step {
            Step::Patch { module, stock, image } => {
                if let Some(stock) = stock {
                    fs::create_dir_all(&folder).map_err(|e| io_err("cannot create", &folder, e))?;
                    write(&backup_of(dir, module), &stock)?;
                }
                write(&dir.join(module.file()), &image)?;
            }
            Step::Restore(module) => {
                let path = dir.join(module.file());
                rename_retry(&backup_of(dir, module), &path).map_err(|e| io_err("cannot restore", &path, e))?;
            }
            Step::DropBackup(module) => {
                let backup = backup_of(dir, module);
                fs::remove_file(&backup).map_err(|e| io_err("cannot remove", &backup, e))?;
            }
        }
    }
    if fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none()) {
        fs::remove_dir(&folder).map_err(|e| io_err("cannot remove", &folder, e))?;
    }
    Ok(lines)
}

/// `--audit`: checks each patched module against its backup and the current dreamseeker.exe, and reports every patch.
/// Read-only. `Err` holds the reports with every problem.
pub fn audit(dir: &Path) -> Result<String, Failure> {
    let mut patched = Vec::new();
    for module in Module::ALL {
        let path = dir.join(module.file());
        if path.is_file() {
            let live = read(&path)?;
            if is_patched(&live) {
                patched.push((module, live));
            }
        }
    }
    if patched.is_empty() {
        return Ok(format!("no DreamSeeker patches in {}: nothing to audit", dir.display()));
    }
    let exe = dir.join(EXE);
    if !exe.is_file() {
        return Err(format!("DreamSeeker patches found, but no {EXE} in {} to check their gates", dir.display()).into());
    }
    let exe = audit::exe_id(&read(&exe)?)?;
    let (mut out, mut failed) = (Vec::new(), false);
    for (module, live) in patched {
        let backup = backup_of(dir, module);
        let result = if backup.is_file() {
            audit::audit(&live, &read(&backup)?, module, exe)
        } else {
            Err(format!("{} is missing", backup.display()))
        };
        match result {
            Ok(report) => out.push(format!("{}: passed\n{report}", module.file())),
            Err(problems) => {
                failed = true;
                out.push(format!("{}: FAILED\n{problems}", module.file()));
            }
        }
    }
    let text = out.join("\n\n");
    if failed { Err(text.into()) } else { Ok(text) }
}
