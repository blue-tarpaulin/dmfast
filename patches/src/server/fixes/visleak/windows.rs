//! `visleak`, Windows: the emptied-vis_locs handler's entry goes to [`emptied`], which handles images.

use crate::base::Result;
use crate::base::code::{self, Foreign, live_u32};
use crate::base::feature::Ctx;
use crate::base::sync::Installed;

/// A value's type byte for an image.
const IMAGE: u32 = 0x0D;
/// An image's vis record: its vis_contents list, then its vis_locs list.
const RECORD: usize = 0x24;

/// The handler (cdecl: a value's type and data): push ebp ; mov ebp,esp ; push ebx ; mov ebx,[ebp+8] ;
/// movzx eax,bl ; sub eax,1 ; je obj ; sub eax,1 ; je mob ; sub eax,1 ; jne done ; (turf) mov ecx,[ebp+0Ch] ;
/// cmp ecx,[turf_count] ; jae done ; mov eax,[turfs] ; mov ecx,[eax+ecx*4] ; test ecx,ecx ; je done ;
/// add ecx,50h ; pop ebx ; pop ebp ; jmp release
const EMPTIED: &str = "55 8B EC 53 8B 5D 08 0F B6 C3 83 E8 01 74 ?? 83 E8 01 74 ?? 83 E8 01 75 ?? 8B 4D 0C 3B 0D ?? ?? \
                       ?? ?? 73 ?? A1 ?? ?? ?? ?? 8B 0C 88 85 C9 74 ?? 83 C1 50 5B 5D E9";
const EMPTIED_RELEASE: usize = 0x36;
/// push ebp ; mov ebp,esp ; push ebx ; mov ebx,[ebp+8]: 7 position-independent bytes.
const ENTRY: usize = 7;
/// The release of a record's vis_locs list (fastcall, the record): push esi ; push edi ; mov edi,ecx ;
/// mov esi,[edi+4] ; test esi,esi ; jne ; pop edi ; xor al,al ; pop esi ; ret ; mov eax,[pool_count] ;
/// cmp eax,100h ; jae free ; cmp dword [esi+8],80h ; ja free
const RELEASE_IDA: &str = "56 57 8B F9 8B 77 04 85 F6 75 05 5F 32 C0 5E C3 A1 ?? ?? ?? ?? 3D 00 01 00 00 73 ?? 81 7E 08 \
                           80 00 00 00 77";
/// The image case of the emptied-vis_contents handler: mov esi,[ebp+0Ch] ; cmp esi,[image_count] ; jae done ;
/// mov eax,[images] ; mov ecx,[eax+esi*4] ; test ecx,ecx ; je done ; add ecx,24h ; call release_contents
const IMAGES: &str = "8B 75 0C 3B 35 ?? ?? ?? ?? 73 ?? A1 ?? ?? ?? ?? 8B 0C B0 85 C9 74 ?? 83 C1 24 E8";
const IMAGES_COUNT: usize = 5;
const IMAGES_ARRAY: usize = 12;

type Emptied = unsafe extern "C" fn(u32, u32);

/// BYOND's release of a record's vis_locs list, and the image table's count and array globals.
struct Found {
    release: Foreign<unsafe extern "fastcall" fn(usize) -> u8>,
    count: usize,
    array: usize,
}

static FOUND: Installed<Found> = Installed::new();
static ORIGINAL: Foreign<Emptied> = Foreign::new();

/// A thing's vis_locs list emptied: BYOND's handler, or for an image, the list's release.
unsafe extern "C" fn emptied(kind: u32, id: u32) {
    if kind & 0xFF != IMAGE {
        // SAFETY: BYOND's call, passed on.
        return unsafe { ORIGINAL.get()(kind, id) };
    }
    let found = FOUND.get();
    // SAFETY: BYOND's image table and an image's vis record; BYOND's release on it.
    unsafe {
        if id >= live_u32(found.count) {
            return;
        }
        let image = code::read::<usize>(live_u32(found.array) as usize + 4 * id as usize);
        if image != 0 {
            found.release.get()(image + RECORD);
        }
    }
}

pub(super) fn install(cx: &Ctx) -> Result<String> {
    let core = cx.core();
    let handler = core.find_unique(EMPTIED, "emptied vis_locs handler")?;
    let release = core.rel32(handler + EMPTIED_RELEASE)?;
    core.require(release, RELEASE_IDA, "vis_locs release")?;
    let images = core.find_unique(IMAGES, "emptied vis_contents handler: images")?;
    FOUND.commit(
        cx,
        Found {
            release: Foreign::at(release),
            count: core.ptr_at(images + IMAGES_COUNT)?,
            array: core.ptr_at(images + IMAGES_ARRAY)?,
        },
    );
    ORIGINAL.set(core.hook(handler, ENTRY, emptied as Emptied)?);
    Ok(core.report(&[handler]))
}
