//! `mapcolors` (forum report 2964093): `icon.MapColors()` with 16 arguments is no longer a "list index out of
//! bounds" runtime error.
//!
//! The built-in `/icon` DM source (a table of NUL-terminated chunks the compiler parses first) passes `args[13]` to
//! `args[20]` for more than 12 arguments. The table entry is pointed at a copy with a branch for 13 to 16 arguments,
//! passing `0, 0, 0, 0` as the offset row. The proc is compiled into the `.dmb`, so the fix needs only the compiler.

use std::sync::OnceLock;

use crate::base::feature::{Ctx, Feature, Hosts, Tier, install};
use crate::base::{Result, bail};

pub static MAPCOLORS: Feature = Feature::fix(
    Hosts::COMPILER,
    "mapcolors",
    "icon.MapColors accepts 16 arguments",
    Tier::Safe,
    install!(windows: install, linux: install),
);

const MARK: &[u8] = b"\n// icons\n#define ICON_ADD 0\n";
const OLD: &str = "\t\t\telse _dm_icon_map_colors(icon,a,b,c,d,e,f,g,h,i,j,k,l,args[13],";
const NEW: &str = "\t\t\telse if(args.len <= 16) _dm_icon_map_colors(icon,a,b,c,d,e,f,g,h,i,j,k,l,args[13],args[14],args[15],args[16],0,0,0,0)\n";

/// The replacement chunk (NUL-terminated); the compiler reads it for the life of the process.
static FIXED: OnceLock<Vec<u8>> = OnceLock::new();

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let chunk = core.find_bytes(MARK)?.unique("icon DM source")?;
    let Ok(text) = std::str::from_utf8(core.c_str(chunk)?) else { bail!("icon DM source is not UTF-8") };
    if text.matches(OLD).count() != 1 {
        bail!("icon DM source: MapColors not as expected");
    }
    let new = FIXED.get_or_init(|| format!("{}\0", text.replacen(OLD, &format!("{NEW}{OLD}"), 1)).into_bytes());
    let slot = core.find_data(&(chunk as u32).to_le_bytes())?.unique("icon DM source table entry")?;
    core.write_data(slot, &(new.as_ptr() as u32).to_le_bytes())?;
    Ok(format!("table entry at +{:#x}", slot - core.base))
}
