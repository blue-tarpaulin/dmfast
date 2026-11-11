//! DreamSeeker's patches (`dmfast_seeker`), written into byondcore.dll and byondwin.dll on disk (DreamSeeker loads
//! no other DLL), with the stock DLLs kept in `bin/backup/`. dmfast.dll itself never goes into dreamseeker.exe. A
//! build without private patches carries none, and its install undoes an earlier install's.
//!
//! Install and uninstall first work out a [`Plan`] (reading, applying, and checking the DLLs aren't in use), so a
//! failure stops them before anything is written, then [`carry_out`] writes it.

use std::fs;
use std::path::{Path, PathBuf};

use dmfast_format::manifest::CODE_SECTION;
use dmfast_seeker::patch::{Dll, Patch};
use dmfast_seeker::{apply, audit, author};

use super::{ensure_not_in_use, pe};
use crate::files::{Failure, io_err, read, rename_retry, write};
use crate::fixes;

/// The private patches: each folder's key, compiled object, and spec (none in a build without them).
const CLIENT: &[(&str, &[u8], &str)] = include!(concat!(env!("OUT_DIR"), "/client.rs"));
/// The program whose gates open; its PE identity goes into every patched DLL.
const EXE: &str = "dreamseeker.exe";
/// The folder in `bin` holding the stock DLLs (and nothing else).
const BACKUP: &str = "backup";

fn backup_of(dir: &Path, dll: Dll) -> PathBuf {
    dir.join(BACKUP).join(dll.file())
}

fn is_patched(data: &[u8]) -> bool {
    pe::has_section(data, &CODE_SECTION)
}

/// What install or uninstall does to one DLL.
enum Step {
    /// Keep `stock` as the backup (when it isn't one yet), then write `image` as the DLL.
    Patch { dll: Dll, stock: Option<Vec<u8>>, image: Vec<u8> },
    /// Put the backup back as the DLL.
    Restore(Dll),
    /// The DLL is stock: its backup, from before a BYOND update, goes.
    DropBackup(Dll),
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

/// Whether DreamSeeker is patched now (byondcore.dll or byondwin.dll has patches).
pub fn client_patched(dir: &Path) -> bool {
    Dll::ALL.iter().any(|dll| read(&dir.join(dll.file())).is_ok_and(|d| is_patched(&d)))
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

/// Install: with `client`, the patches the saved fix selection picks (or [`CLIENT_DEFAULT`]'s without one), applied
/// to each DLL's stock bytes (its backup if it is patched, else the DLL itself). A DLL no picked patch applies to (all
/// of them without `client`) goes back to stock.
pub fn plan_install(dir: &Path, client: bool) -> Result<Plan, Failure> {
    let selection = if !client {
        Vec::new()
    } else {
        match fixes::current(dir)? {
            None => vec![CLIENT_DEFAULT.into()],
            Some(v) => fixes::entries(&v)?,
        }
    };
    let patches: Vec<Patch> = embedded()?.into_iter().filter(|p| p.wanted(&selection)).collect();
    let mut plan = Plan { steps: Vec::new(), lines: Vec::new(), patched: false };
    if client && patches.is_empty() {
        plan.lines.push("DreamSeeker: the fix selection picks none of its patches ('--help client' lists them)".into());
    }
    let exe = dir.join(EXE);
    let exe_id = if patches.is_empty() {
        None
    } else if exe.is_file() {
        Some(audit::exe_id(&read(&exe)?)?)
    } else {
        plan.lines.push(format!("DreamSeeker patches skipped: no {EXE} in {}", dir.display()));
        None
    };
    for dll in Dll::ALL {
        let path = dir.join(dll.file());
        if !path.is_file() {
            continue;
        }
        let live = read(&path)?;
        let backup = backup_of(dir, dll);
        let patched = is_patched(&live);
        if patched && !backup.is_file() {
            return Err(format!(
                "{} has DreamSeeker patches but {} is missing; reinstall BYOND",
                dll.file(),
                backup.display()
            )
            .into());
        }
        let image = match exe_id {
            Some(exe) => {
                let kept = if patched { Some(read(&backup)?) } else { None };
                let applied = apply::apply(kept.as_deref().unwrap_or(&live[..]), dll, exe, &patches)
                    .map_err(|e| format!("DreamSeeker patches for {}: {e}", dll.file()))?;
                plan.lines.extend(applied.lines.iter().map(|l| format!("DreamSeeker, {}: {l}", dll.file())));
                applied.image
            }
            None => None,
        };
        match image {
            Some(image) if image == live => plan.patched = true,
            Some(image) => {
                ensure_not_in_use(&path)?;
                plan.patched = true;
                plan.steps.push(Step::Patch { dll, stock: (!patched).then_some(live), image });
            }
            None if patched => {
                ensure_not_in_use(&path)?;
                plan.steps.push(Step::Restore(dll));
                plan.lines.push(format!("restored stock {}", dll.file()));
            }
            None if backup.is_file() => plan.steps.push(Step::DropBackup(dll)),
            None => {}
        }
    }
    Ok(plan)
}

/// Uninstall: each patched DLL back from its backup. One whose backup is gone stays as it is, with a warning (its
/// gates still run BYOND's code outside DreamSeeker).
pub fn plan_uninstall(dir: &Path) -> Result<Plan, Failure> {
    let mut plan = Plan { steps: Vec::new(), lines: Vec::new(), patched: false };
    for dll in Dll::ALL {
        let path = dir.join(dll.file());
        if !path.is_file() {
            continue;
        }
        let backup = backup_of(dir, dll);
        if !is_patched(&read(&path)?) {
            if backup.is_file() {
                plan.steps.push(Step::DropBackup(dll));
            }
        } else if backup.is_file() {
            ensure_not_in_use(&path)?;
            plan.steps.push(Step::Restore(dll));
            plan.lines.push(format!("restored stock {}", dll.file()));
        } else {
            plan.lines.push(format!(
                "warning: {} was missing, so {} keeps its DreamSeeker patches; reinstall BYOND to get it stock",
                backup.display(),
                dll.file()
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
            Step::Patch { dll, stock, image } => {
                if let Some(stock) = stock {
                    fs::create_dir_all(&folder).map_err(|e| io_err("cannot create", &folder, e))?;
                    write(&backup_of(dir, dll), &stock)?;
                }
                write(&dir.join(dll.file()), &image)?;
            }
            Step::Restore(dll) => {
                let path = dir.join(dll.file());
                rename_retry(&backup_of(dir, dll), &path).map_err(|e| io_err("cannot restore", &path, e))?;
            }
            Step::DropBackup(dll) => {
                let backup = backup_of(dir, dll);
                fs::remove_file(&backup).map_err(|e| io_err("cannot remove", &backup, e))?;
            }
        }
    }
    if fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none()) {
        fs::remove_dir(&folder).map_err(|e| io_err("cannot remove", &folder, e))?;
    }
    Ok(lines)
}

/// `--audit`: checks each patched DLL against its backup and dreamseeker.exe, and reports every patch. Reads only.
/// `Err` holds the reports with every problem.
pub fn audit(dir: &Path) -> Result<String, Failure> {
    let mut patched = Vec::new();
    for dll in Dll::ALL {
        let path = dir.join(dll.file());
        if path.is_file() {
            let live = read(&path)?;
            if is_patched(&live) {
                patched.push((dll, live));
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
    for (dll, live) in patched {
        let backup = backup_of(dir, dll);
        let result = if backup.is_file() {
            audit::audit(&live, &read(&backup)?, dll, exe)
        } else {
            Err(format!("{} is missing", backup.display()))
        };
        match result {
            Ok(report) => out.push(format!("{}: passed\n{report}", dll.file())),
            Err(problems) => {
                failed = true;
                out.push(format!("{}: FAILED\n{problems}", dll.file()));
            }
        }
    }
    let text = out.join("\n\n");
    if failed { Err(text.into()) } else { Ok(text) }
}
