//! `ctype`, Windows: `call [isX] ; add esp,4 ; test eax,eax` calls [`classify`] instead.

use crate::base::code::{Foreign, hx};
use crate::base::feature::Ctx;
use crate::base::{Result, bail};

/// A CRT classification function, its 0/1 answers for 0..127, the original, and its replacement.
struct Class {
    name: &'static str,
    table: [u8; 128],
    original: Foreign<unsafe extern "C" fn(i32) -> i32>,
    hook: extern "C" fn(i32) -> i32,
}

/// `CLASSES`, each `index name: c => test` with `test` a `bool` of `c: u8`. Indices must count up from 0.
macro_rules! classes {
    ($($i:literal $name:literal: $c:ident => $test:expr),* $(,)?) => {
        static CLASSES: [Class; [$($i),*].len()] = [$(Class {
            name: $name,
            table: {
                let mut t = [0u8; 128];
                let mut i = 0;
                while i < 128 {
                    let $c = i as u8;
                    t[i] = $test as u8;
                    i += 1;
                }
                t
            },
            original: Foreign::new(),
            hook: classify::<$i>,
        }),*];
        const _: () = {
            let mut n = 0;
            $(assert!($i == n, "class indices must count up from 0"); n += 1;)*
        };
    };
}

classes! {
    0 "isalnum": c => c.is_ascii_alphanumeric(),
    1 "isalpha": c => c.is_ascii_alphabetic(),
    2 "isdigit": c => c.is_ascii_digit(),
    // C's isspace also counts \v, which Rust's is_ascii_whitespace doesn't.
    3 "isspace": c => c == b' ' || matches!(c, 0x09..=0x0D),
    4 "isxdigit": c => c.is_ascii_hexdigit(),
    5 "isupper": c => c.is_ascii_uppercase(),
    6 "islower": c => c.is_ascii_lowercase(),
}

/// Replaces class `I`'s CRT function at the redirected call sites.
extern "C" fn classify<const I: usize>(c: i32) -> i32 {
    let class = &CLASSES[I];
    match c {
        0..=127 => i32::from(class.table[c as usize]),
        // SAFETY: the CRT's function from the import slot, with its own argument.
        _ => unsafe { class.original.get()(c) },
    }
}

pub fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let mut patched = Vec::new();
    for class in &CLASSES {
        let Ok(slot) = core.iat_slot(class.name) else { continue };
        // call [isX] ; add esp,4 ; test eax,eax
        let sites = core.find_all(&format!("FF 15 {} 83 C4 04 85 C0", hx(slot)))?;
        if sites.is_empty() {
            continue;
        }
        class.original.set(core.live_u32(slot)? as usize);
        for &site in &sites {
            core.call_iat_to(site, slot, class.hook as *const () as usize)?;
        }
        patched.push(format!("{} {}", class.name, sites.len()));
    }
    if patched.is_empty() {
        bail!("no classification call sites");
    }
    Ok(patched.join(", "))
}
