//! `mapcolors` (forum report 2964093): `icon.MapColors()` with 16 arguments is no longer a "list index out of
//! bounds" runtime error.
//!
//! The built-in `/icon` DM source passes `args[13]` to `args[20]` for more than 12 arguments. Its chunk is replaced
//! by a copy with a branch for 13 to 16 arguments, passing `0, 0, 0, 0` as the offset row. The proc is compiled into
//! the `.dmb`, so the fix needs only the compiler.

use std::sync::OnceLock;

use super::replace_source;
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

static FIXED: OnceLock<Vec<u8>> = OnceLock::new();

fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let slot = replace_source(core, MARK, "icon DM source", &FIXED, |text| {
        if text.matches(OLD).count() != 1 {
            bail!("icon DM source: MapColors not as expected");
        }
        Ok(text.replacen(OLD, &format!("{NEW}{OLD}"), 1))
    })?;
    Ok(format!("table entry at +{:#x}", slot - core.base))
}
