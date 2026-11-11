"""Maintains patches/src/deps.rs: the dependency guards of patches that mirror BYOND data (see
base/src/deps/mod.rs).

For each function in MANIFEST, deps.rs holds what has been accepted: a fingerprint of each known form (addresses
wildcarded: relocated operands and the rel32 of calls and jumps that leave the function), the reference
counts of the writable globals it uses, and (for writers that aren't hooked) its number of call sites. A
fingerprint is the form's first PREFIX bytes as a signature, its length, its wildcarded spans, and an FNV-1a
64 hash of its other bytes; no BYOND code is stored. To diff and align forms, the tool rebuilds each form from
one of the builds listed above it (which must be in <builds dir>) and checks it against the hash.

    py tools/depsig.py <builds dir>                  check every build against deps.rs
    py tools/depsig.py <builds dir> --accept <v,..>  also accept what those builds changed

<builds dir>/<version>/ holds byondcore.dll and libbyond.so from that version's zips. A build that doesn't
match is reported with an instruction diff and the counts that changed, and nothing is written: only
--accept adds a form or a count, after the patches have been checked against that build. Entries new to
MANIFEST are created from every build in the folder (writing the entry is the review). Entries removed
from MANIFEST are dropped. Bookkeeping (which builds each form came from) is updated on every clean run.

Globals of a new entry: every writable global the function's relocated operands point at, except ones
with more than BUSY references (locks, allocator state: not the mirrored data). Their place in a new
form is carried over from the closest known form by aligning the two instruction listings.

needs: pip install capstone pefile pyelftools
"""
import collections, difflib, glob, os, re, struct, sys
import capstone

REF = "516.1688"  # build the MANIFEST addresses are in
MAX_LEN = 0x1000  # bigger functions change too often and cost too much to carry
BUSY = 200
PREFIX = 48  # leading bytes kept as a form's signature, and used to find a changed function
ABSENT = 0xFFFF  # a form's offset for a global it doesn't address (runtime: not checked through it)

# (os, patch keys, name, start address in REF, options). Options: callers=True also guards the number of
# call sites (writers that patches don't hook); globals=False skips globals (a function that touches every
# table, where the guarded data's own functions cover it); max_len raises MAX_LEN for one function; split=True
# also ends a Windows function at a `push ebp ; mov ebp,esp` right after a return (no int3 padding before it).
MANIFEST = [
    # compiler string table: intern (strtab replaces its walk, strings its free-slot scan), the .dmb save
    # (unlinks and frees strings; ftell: its loop touches the .dmb only through the string writer), the global
    # reset (frees the table)
    ("win", ("strings", "strtab"), "string intern", 0x100FCED0, {}),
    ("win", ("strings", "strtab", "ftell"), "string table save", 0x10100070, {}),
    ("win", ("strings", "strtab", "slots"), "table reset", 0x100FF2B0, {"globals": False, "callers": True}),
    ("lin", ("strings", "strtab"), "string intern", 0x1E0BF0, {}),
    ("lin", ("strings", "strtab"), "string table save", 0x1E0FB0, {}),
    ("lin", ("strings", "strtab"), "string compare", 0x4B4260, {}),
    ("lin", ("strings", "strtab", "slots"), "table reset", 0x1DF910, {"globals": False, "callers": True}),
    # other compiler indexes and caches
    # name filter: only inserts can make it skip wrongly; every call of the insert must be one of the two
    # hooked indexer sites, a new caller would be an unrecorded insert
    ("win", ("filter",), "name filter insert", 0x10100AC0, {"callers": True, "split": True}),
    ("win", ("parent",), "parent lookup", 0x100CE6E0, {}),
    ("win", ("vecgrow",), "vector growth", 0x100C92A0, {}),
    ("win", ("resources",), "resource lookup", 0x1023E128, {}),
    ("win", ("resources",), "resource removal", 0x1023F8B0, {"callers": True}),
    ("win", ("lists",), "list intern", 0x100C9920, {}),
    ("win", ("listinit",), "list initializer intern", 0x100FD060, {}),
    ("win", ("protos",), "proto lookup", 0x10102FE0, {}),
    ("win", ("internidx",), "identifier intern", 0x100BE0E0, {}),
    ("lin", ("parent",), "parent lookup", 0x1BC9F0, {}),
    ("lin", ("children",), "child lookup", 0x1BBF90, {}),
    ("lin", ("vecgrow",), "vector growth", 0x1B5AE0, {}),
    ("lin", ("resources",), "resource lookup", 0x4A7F90, {}),
    ("lin", ("lists",), "list intern", 0x1B57A0, {}),
    ("lin", ("listinit",), "list initializer intern", 0x1E2120, {}),
    ("lin", ("protos",), "proto lookup", 0x18ADD0, {}),
    ("lin", ("internidx",), "identifier intern", 0x1CDCC0, {}),
    # ftell caches the string writer's position across its calls from the save loop: between its ftell calls it
    # must write only through the replaced writers (the next function follows it without padding)
    ("win", ("ftell",), "string writer", 0x1024C280, {"split": True}),
    # leafskip returns at once from the compile pass on a finished leaf: every handler in it must ignore those,
    # and no caller may use its return value
    ("win", ("leafskip",), "compile pass", 0x100ACED0, {"callers": True}),
    ("lin", ("leafskip",), "compile pass", 0x1A5320, {"callers": True}),
    # dirs replays the directory listing from a cache and defers its ChangeDir: nothing else in the listing may
    # use the directory or change an entry, and ChangeDir must do nothing more (its cached cwd: who reads it)
    ("lin", ("dirs",), "directory listing", 0x4C30F0, {"max_len": 0x1400}),
    ("lin", ("dirs",), "change dir", 0x4AAA90, {}),
    # fastexit ends the compiler instead of its teardown: the builder destructor it skips (Linux); the resource
    # lock release it runs itself, which must unlink each entry, and the lock list's users (Windows)
    ("win", ("fastexit",), "resource lock release", 0x1029FA20, {}),
    ("lin", ("fastexit",), "builder destructor", 0x1E5B50, {}),
    # runtime string tree: lookup (replaced), removal, intern (stores the new entry after a miss), rebuild and
    # its tree builder (relink), the .dmb string load (writes the table) and its per-entry pass (strtable)
    ("win", ("strtree", "strtable"), "string lookup", 0x10213580, {}),
    ("win", ("strtree", "strtable"), "string removal", 0x10221DE0, {}),
    ("win", ("strtree", "strtable"), "runtime string intern", 0x101EE960, {}),
    ("win", ("strtree", "strtable"), "string tree rebuild", 0x10213890, {"callers": True}),
    ("win", ("strtree", "strtable"), "string tree builder", 0x102235F0, {"callers": True}),
    ("win", ("strtree", "strtable", "dmbread"), "string table load", 0x1011A090, {"callers": True}),
    ("win", ("strtable",), "string load pass", 0x10207A00, {"callers": True}),
    ("lin", ("strtree",), "runtime string intern", 0x2D75A0, {}),
    ("lin", ("strtree",), "string load pass", 0x2E3920, {"callers": True}),
    ("lin", ("strtree",), "string find", 0x2E2B10, {}),
    ("lin", ("strtree",), "string removal", 0x2E3B30, {}),
    ("lin", ("strtree",), "string tree rebuild", 0x2D51F0, {"callers": True}),
    ("lin", ("strtree",), "string tree builder", 0x2D4EA0, {"callers": True}),
    # dmbread answers the .dmb string reader's ftell from its own count: between ftell calls the reader and its
    # only caller, the string table load (above), must read the stream only through the replaced helpers
    ("win", ("dmbread",), ".dmb string reader", 0x1024C1E0, {}),
    # iconstates skips the new-key scan for the icon_states builder's list: the builder must add to it only
    # through the associative set
    ("win", ("iconstates",), "icon_states builder", 0x101284C0, {}),
    ("lin", ("iconstates",), "icon_states builder", 0x426890, {}),
    # other runtime patches
    ("win", ("procs",), "proc table find-or-add", 0x10226960, {}),
    ("win", ("sortmap",), "sorted map merge", 0x1020F2F0, {"callers": True}),
    ("win", ("sortmap",), "sorted map insert", 0x101EE870, {}),
    ("win", ("crc",), "CRC-32", 0x102455C0, {}),
    ("win", ("varlist",), "var list merge", 0x10117EC0, {}),
    ("win", ("varlist",), "var list pass", 0x1011A960, {}),
    ("win", ("dmicmp",), "icon state name compare", 0x100A5E20, {"split": True}),
    ("lin", ("procs",), "proc table find-or-add", 0x2ED8C0, {}),
    ("lin", ("sortmap",), "sorted map merge", 0x2ECBD0, {"callers": True}),
    ("lin", ("sortmap",), "sorted map insert", 0x2ECAC0, {}),
    ("lin", ("crc",), "CRC-32", 0x4A4C30, {}),
    # argload rewrites these prologues in place: the rest of each function must not branch into them or rely
    # on what the rewrite changes (assign: its call's return address, the dead arg slot it writes early)
    ("lin", ("argload",), "value IncRef", 0x2EE220, {"globals": False}),
    ("lin", ("argload",), "value DecRef", 0x30D6F0, {"globals": False}),
    ("lin", ("argload",), "value assign", 0x30E2C0, {"globals": False}),
    # seenclear sweeps a record tree's node pool instead of its list: every allocated node must be linked
    # into the list (allocator: its call sites) and every free must mark the slot free (the pool's frees)
    ("win", ("seenclear",), "record pool allocator", 0x1012B4E0, {"globals": False, "callers": True}),
    ("win", ("seenclear",), "record tree erase", 0x1012CDF0, {"globals": False}),
    ("win", ("seenclear",), "record tree erase 2", 0x1012CFD0, {"globals": False}),
    ("lin", ("seenclear",), "record pool allocator", 0x4249A0, {"globals": False, "callers": True}),
    ("lin", ("seenclear",), "record tree erase", 0x424770, {"globals": False}),
    # mapskip answers the record check's unchanged path at its entry: the rest of the check (no branch back
    # into its first bytes, the unchanged path writes only the seen flag) and the tree's read-only find
    ("win", ("mapskip",), "client record check", 0x1012AA20, {"globals": False}),
    ("win", ("mapskip",), "client record find", 0x1012C390, {"globals": False}),
    ("lin", ("mapskip",), "client record check", 0x423330, {"globals": False}),
    ("lin", ("mapskip",), "client record find", 0x424E10, {"globals": False}),
    # getvar rewrites these prologues (the var read's through a stub that jumps back to its first call): the rest
    # of each function must not branch into them. GetVariable is one big switch, but has had few forms.
    ("lin", ("getvar",), "GetVariable", 0x3C25A0, {"globals": False, "max_len": 0x3000}),
    ("lin", ("getvar",), "user var read", 0x2EBF40, {"globals": False}),
    # varread caches what a type's initial-value and declaration searches answer: their id arrays are only written
    # by the .dmb load (which empties the cache), its unload, and the id-array intern (guarded by its call sites)
    ("win", ("varread",), "user var read", 0x102030D0, {}),
    ("win", ("varread",), "id-array intern", 0x10226780, {"callers": True}),
    # turfvar links new turf var records at their chain's tail: find-or-create must stay the only code that links
    # them (its globals' reference counts: the bucket table, mask, record counter) and the walk over all records
    # (the only reader that sees chain order, the reference sweeps) must not gain callers
    ("win", ("turfvar",), "turf var find-or-create", 0x101EECF0, {}),
    ("win", ("turfvar",), "turf var record walk", 0x10216330, {"globals": False, "callers": True}),
    ("lin", ("turfvar",), "turf var find-or-create", 0x2D2BE0, {}),
    ("lin", ("turfvar",), "turf var record walk", 0x2E7650, {"globals": False, "callers": True}),
    # turfpf prefetches what the turf contents lookup will touch, from its globals (turf count, flags, mask,
    # buckets), and steers by the turf scan's frame slots (Linux: the scan is inlined in a 0x23D1-byte function,
    # carried with max_len). The object/mob by-id reads are matched whole at install (too small to locate)
    ("win", ("turfpf",), "map turf scan", 0x101D7560, {}),
    ("win", ("turfpf",), "turf contents lookup", 0x10204120, {"split": True}),
    ("lin", ("turfpf",), "map turf scan", 0x3512A0, {"max_len": 0x3000}),
    ("lin", ("turfpf",), "turf contents lookup", 0x2E46F0, {}),
    # locidx indexes the vector a view/range is built in: the append must stay its only writer between lookups
    # (the append itself, the unique append / the gatherer whose scans it replaces, and who calls those)
    ("win", ("locidx",), "loc list append", 0x10184270, {"globals": False}),
    ("win", ("locidx",), "loc list unique append", 0x10184310, {"globals": False, "callers": True}),
    ("lin", ("locidx",), "loc list append", 0x2C6A50, {"globals": False}),
    ("lin", ("locidx",), "turf/area gatherer", 0x2C6F20, {"globals": False, "callers": True}),
    # iconchain walks stored icon operation lists through BYOND's walker for the resources they load and, while it
    # draws, answers the server icon context's look-up with its own copies (the look-up: what it caches, at which
    # nesting, and hands out)
    ("win", ("iconchain",), "icon operation walker", 0x100A3440, {}),
    ("win", ("iconchain",), "server icon look-up", 0x101264C0, {}),
    ("lin", ("iconchain",), "icon operation walker", 0x572BE0, {}),
    ("lin", ("iconchain",), "server icon look-up", 0x425BF0, {}),
]

# A unique pattern inside a function whose leading bytes vary between builds, to find it in a build no known
# form matches (review/--accept only; the runtime still needs a known form).
ANCHOR = {
    ("lin", "record tree erase"):  # seenclear's free signature
        "BD FF FF FF FF C7 43 10 00 00 00 00 66 89 6B 18 C7 43 0C 00 00 00 00 C7 43 08 00 00 00 00 C7 03 00 00 00 00 "
        "8B 47 0C 89 43 04 85 C0 74 02 89 18 83 6F 10 01 8B 4F 24 89 5F 0C 8B 43 14 D3 E8 89 C6 89 44 24 ?? "
        "8B 47 18 8D 14 F0 83 6A 04 01 8B 47 14 75 ?? 8D B6 00 00 00 00 83 F8 01 76 ?? 8B 4F 18 8D 14 C5 F8 FF FF FF "
        "8D 2C 11 8B 75 04 85 F6 75 ?? 8B 74 11 FC 85 F6 75 ?? 8B 47 20 8B 6D 00 85 C0 74 ?? 6B C0 5C",
    ("lin", "compile pass"):  # leafskip's pass function: a sequence 515 shares (its leading bytes differ)
        "FF FF 83 EC 0C FF 75 D4 89 55 A4 E8 ?? ?? ?? ?? 83 C4 10 8B 55 A4 0F B7 C8 89 4D A8 DB 45 A8 D9 45 D4 D9 C9 "
        "DF E9 DD D8",
}

md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
md.detail = True
NOPS = set(b"\x90\x8d\xb4\x26\x00\x76\xb6\x66\x2e\x0f\x1f\x84\x40\x44\xbc\x27")


def fde_bounds(eh, eh_va):
    """{function start: end} from an ELF .eh_frame (CIEs with 'z' augmentation and an sdata4 FDE encoding,
    absolute or pc-relative, as GCC emits for i386)."""
    out, enc, i = {}, {}, 0
    while i + 4 <= len(eh):
        length = struct.unpack_from("<I", eh, i)[0]
        if length == 0:
            break
        body, nxt = i + 4, i + 4 + length
        cie_id = struct.unpack_from("<I", eh, body)[0]
        if cie_id == 0:  # CIE: version, augmentation string, code/data align, return reg, augmentation data
            p = body + 5
            aug = eh[p:eh.index(b"\0", p)].decode()
            p += len(aug) + 1
            for _ in range(2):  # uleb code align, sleb data align
                while eh[p] & 0x80:
                    p += 1
                p += 1
            p += 1  # return register (version 1: one byte)
            fe = 0
            if aug.startswith("z"):
                while eh[p] & 0x80:
                    p += 1
                p += 1
                for c in aug[1:]:
                    if c == "R":
                        fe = eh[p]; p += 1
                    elif c == "L":
                        p += 1
                    elif c == "P":
                        pe = eh[p]; p += 1 + {0x00: 4, 0x03: 4, 0x0B: 4, 0x1B: 4, 0x9B: 4}[pe]
            enc[i] = fe
        else:
            fe = enc.get(body - cie_id, 0)
            if fe & 0x0F in (0x00, 0x03, 0x0B):  # absptr / udata4 / sdata4
                start = struct.unpack_from("<i" if fe & 0x0F == 0x0B else "<I", eh, body + 4)[0]
                if fe & 0x70 == 0x10:  # pc-relative
                    start += eh_va + body + 4
                start &= 0xFFFFFFFF
                out[start] = start + struct.unpack_from("<I", eh, body + 8)[0]
        i = nxt
    return out


class Image:
    """One build's core library: bytes, address mapping, relocations, and the counts the runtime takes."""

    def __init__(self, os_, path):
        self.os = os_
        self.fde_end = {}
        self.data = d = open(path, "rb").read()
        self.relocs = set()  # every relocated byte offset (wildcarded in patterns)
        self.abs = {}  # file offset of an absolute 32-bit relocation -> the address it holds once loaded
        if os_ == "win":
            import pefile
            p = pefile.PE(path)
            base = p.OPTIONAL_HEADER.ImageBase
            self.to_off = lambda va: p.get_offset_from_rva(va - base)
            self.to_va = lambda off: p.get_rva_from_offset(off) + base
            secs = [(s.VirtualAddress + base, s.Misc_VirtualSize, s.PointerToRawData,
                     min(s.Misc_VirtualSize, s.SizeOfRawData), s.Characteristics) for s in p.sections]
            self.exec = [(o, n) for va, vs, o, n, c in secs if c & 0x20000000]
            self.writable = lambda va: any(s <= va < s + vs and c & 0x80000000 for s, vs, _, _, c in secs)
            for blk in getattr(p, "DIRECTORY_ENTRY_BASERELOC", []):
                for e in blk.entries:
                    if e.type == 3:
                        o = p.get_offset_from_rva(e.rva)
                        self.relocs.add(o)
                        self.abs[o] = struct.unpack_from("<I", d, o)[0]
        else:
            from elftools.elf.elffile import ELFFile
            from elftools.elf.relocation import RelocationSection
            with open(path, "rb") as f:
                elf = ELFFile(f)
                loads = [(s["p_vaddr"], s["p_offset"], s["p_filesz"], s["p_memsz"], s["p_flags"])
                         for s in elf.iter_segments() if s["p_type"] == "PT_LOAD"]
                eh = elf.get_section_by_name(".eh_frame")
                self.fde_end = fde_bounds(eh.data(), eh["sh_addr"]) if eh else {}
                rels, absolute = [], []
                for sec in elf.iter_sections():
                    if not isinstance(sec, RelocationSection):
                        continue
                    syms = elf.get_section(sec["sh_link"])
                    for r in sec.iter_relocations():
                        rels.append(r["r_offset"])
                        if sec.name == ".rel.dyn" and r["r_info_type"] in (1, 8):  # R_386_32, R_386_RELATIVE
                            s = None
                            if r["r_info_type"] == 1:
                                sym = syms.get_symbol(r["r_info_sym"])
                                s = None if sym["st_shndx"] == "SHN_UNDEF" else sym["st_value"]
                            absolute.append((r["r_offset"], r["r_info_type"], s))

            def to_off(va):
                for v, o, n, _, _ in loads:
                    if v <= va < v + n:
                        return va - v + o
                raise ValueError(hex(va))

            def to_va(off):
                for v, o, n, _, _ in loads:
                    if o <= off < o + n:
                        return off - o + v
                raise ValueError(hex(off))
            self.to_off, self.to_va = to_off, to_va
            self.exec = [(o, n) for _, o, n, _, fl in loads if fl & 1]
            self.writable = lambda va: any(v <= va < v + m and fl & 2 for v, _, _, m, fl in loads)
            for r in rels:
                try:
                    self.relocs.add(to_off(r))
                except ValueError:
                    pass
            for va, typ, s in absolute:
                try:
                    o = to_off(va)
                except ValueError:
                    continue
                a = struct.unpack_from("<I", d, o)[0]
                if typ == 8:
                    self.abs[o] = a
                elif s is not None:
                    self.abs[o] = (s + a) & 0xFFFFFFFF
        in_exec = lambda o: any(s <= o and o + 4 <= s + n for s, n in self.exec)
        # Code references per address (the runtime counts relocations inside the executable ranges).
        self.refs = collections.Counter(v for o, v in self.abs.items() if in_exec(o))
        # `call rel32` sites per target address, over the executable ranges.
        self.calls = collections.Counter()
        for s, n in self.exec:
            base_va = self.to_va(s)
            for m in re.finditer(b"\xe8", d[s:s + n - 4]):
                i = m.start()
                rel = struct.unpack_from("<i", d, s + i + 1)[0]
                self.calls[(base_va + i + 5 + rel) & 0xFFFFFFFF] += 1

    def u32(self, va):
        return struct.unpack_from("<I", self.data, self.to_off(va))[0]


def function(img, start, split=False):
    off = img.to_off(start)
    d = img.data
    out, reach = [], start
    cap = img.fde_end.get(start)  # Linux: never past the function's own unwind entry
    for ins in md.disasm(d[off:off + 0x8000], start):
        if ins.mnemonic == "int3" or (cap is not None and ins.address >= cap):
            break
        out.append(ins)
        if ins.mnemonic.startswith("j") and ins.operands and ins.operands[0].type == capstone.x86.X86_OP_IMM:
            t = ins.operands[0].imm
            if start < t < start + 0x8000:
                reach = max(reach, t)
        end = ins.address + ins.size
        if ins.mnemonic in ("ret", "jmp") and end > reach:
            e = img.to_off(end)
            if img.os == "win" and (d[e:e + 2] == b"\xcc\xcc" or split and d[e:e + 3] == b"\x55\x8b\xec"):
                break
            if img.os == "lin" and all(b in NOPS for b in d[e:img.to_off((end + 15) & ~15)]):
                break
    return out


def pattern(img, start, split=False):
    """(bytes with wildcards as 0, fixed flags) of the function at `start`."""
    insns = function(img, start, split)
    hi = insns[-1].address + insns[-1].size
    raw, fixed = bytearray(), []
    for ins in insns:
        b = bytes(ins.bytes)
        off = img.to_off(ins.address)
        mask = [True] * len(b)
        for i in range(-3, len(b)):  # from -3: a relocation can start in the previous (mis)decoded instruction
            if off + i in img.relocs:
                for j in range(max(i, 0), min(i + 4, len(b))):
                    mask[j] = False
        if (ins.mnemonic == "call" or ins.mnemonic.startswith("j")) and ins.operands \
                and ins.operands[0].type == capstone.x86.X86_OP_IMM and not (start <= ins.operands[0].imm < hi) \
                and len(b) >= 5:
            for j in range(len(b) - 4, len(b)):
                mask[j] = False
        raw += bytes(x if m else 0 for x, m in zip(b, mask))
        fixed += mask
    return bytes(raw), fixed


def matches(img, raw, fixed):
    """Addresses where the pattern occurs in the executable ranges (as the runtime searches)."""
    rx = re.compile(b"".join(re.escape(bytes([b])) if f else b"." for b, f in zip(raw, fixed)), re.S)
    out = []
    for s, n in img.exec:
        out += [img.to_va(m.start()) for m in rx.finditer(img.data, s, s + n)]
    return out


def abs_sites(img, start, length):
    """(offset in the function, address held) of the absolute relocations inside it, in order."""
    o0 = img.to_off(start)
    return [(o - o0, v) for o, v in sorted(img.abs.items()) if o0 <= o < o0 + length]


NUMBER = re.compile(r"\b(0x[0-9a-f]+|[0-9]+)\b")


def keyed(insns):
    """Instructions with every number (addresses, displacements, immediates) made equal."""
    return [f"{i.mnemonic} {NUMBER.sub('#', i.op_str)}" for i in insns]


def listing(raw):
    """Instructions of a stored form (wildcards decode as zeros; lengths are unaffected)."""
    return list(md.disasm(raw, 0))


def carry_offsets(src_raw, offsets, dst_img, dst_va, split=False):
    """Offsets of the same globals in another form of the function, by aligning the two listings; None for
    a global whose instruction has no counterpart."""
    a, b = listing(src_raw), function(dst_img, dst_va, split)
    sm = difflib.SequenceMatcher(None, keyed(a), keyed(b), autojunk=False)
    pair = {}
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            pair.update({i1 + k: j1 + k for k in range(i2 - i1)})
    sites = {o for o, _ in abs_sites(dst_img, dst_va, b[-1].address + b[-1].size - dst_va)}
    out = []
    for off in offsets:
        if off is None:  # absent from the source form too
            out.append(None)
            continue
        k = next((k for k, ins in enumerate(a) if ins.address <= off < ins.address + ins.size), None)
        new = None
        if k in pair:
            n = b[pair[k]].address - dst_va + off - a[k].address
            new = n if n in sites else None
        out.append(new)
    return out


def diff(src_raw, img, va, split=False, limit=40):
    a, b = keyed(listing(src_raw)), keyed(function(img, va, split))
    lines = [l for l in difflib.unified_diff(a, b, lineterm="", n=1) if not l.startswith(("---", "+++"))]
    return lines[:limit] + ([f"... {len(lines) - limit} more"] if len(lines) > limit else [])


def fnv1a(data, holes):
    """FNV-1a 64 over `data`, skipping the (offset, length) spans in `holes`."""
    h, at = 0xCBF29CE484222325, 0
    for off, n in list(holes) + [(len(data), 0)]:
        for b in data[at:off]:
            h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
        at = off + n
    return h


def spans(fixed):
    """(offset, length) of each run of wildcards."""
    out, i = [], 0
    while i < len(fixed):
        if fixed[i]:
            i += 1
            continue
        j = i
        while j < len(fixed) and not fixed[j]:
            j += 1
        out.append((i, j - i))
        i = j
    return out


class Form:
    """A known form: its full bytes and mask once known (rebuilt from a build), and its fingerprint."""

    def __init__(self, raw, fixed, globals_, builds=()):
        self.raw, self.fixed, self.globals, self.builds = raw, fixed, list(globals_), set(builds)
        if raw is not None:
            self.fingerprint = (self.ida(PREFIX), len(raw), spans(fixed), fnv1a(raw, spans(fixed)))

    def ida(self, n=None):
        return " ".join(f"{b:02X}" if f else "??" for b, f in list(zip(self.raw, self.fixed))[:n])


class Entry:
    def __init__(self, keys, name):
        self.keys, self.name = keys, name
        self.forms, self.globals, self.callers = [], [], None  # globals: accepted count sets
        self.split = False


def find_form(img, fingerprint):
    """Address of the form if it occurs exactly once (its signature matches and the bytes there hash right), as
    the runtime checks it."""
    prefix, n, holes, h = fingerprint
    toks = prefix.split()
    hits = []
    for va in matches(img, bytes(0 if t == "??" else int(t, 16) for t in toks), [t != "??" for t in toks]):
        o = img.to_off(va)
        if any(s <= o and o + n <= s + m for s, m in img.exec) and fnv1a(img.data[o:o + n], holes) == h:
            hits.append(va)
    return hits[0] if len(hits) == 1 else None


def locate(entry, img):
    """(form, address) of the first form that occurs exactly once, as the runtime picks it."""
    for f in entry.forms:
        va = find_form(img, f.fingerprint)
        if va is not None:
            return f, va
    return None


def rebuild(entry, os_, imgs):
    """Fill in the bytes of each form one of whose builds is in `imgs` (a form whose builds all aren't stays
    without: only diffs and new forms need bytes); an error message if such a build doesn't match it."""
    for f in entry.forms:
        have = sorted(f.builds & set(imgs))
        if f.raw is not None or not have:
            continue
        for v in have:
            va = find_form(imgs[v], f.fingerprint)
            if va is None:
                continue
            raw, fixed = pattern(imgs[v], va, entry.split)
            if (len(raw), spans(fixed), fnv1a(raw, spans(fixed))) == f.fingerprint[1:] and \
                    Form(raw, fixed, ()).ida(PREFIX) == f.fingerprint[0]:
                f.raw, f.fixed = raw, fixed
                break
        else:
            return (f"{os_} {entry.name}: can't rebuild the form of builds {' '.join(sorted(f.builds))}: "
                    f"none of {' '.join(have)} matches its fingerprint")
    return None


def find_changed(entry, img):
    """(closest form, address) of a function no known form matches, by its leading bytes: the longest lead
    (down to 16 bytes, if the function changed early) that is still unique; else, on Linux, the function
    holding the entry's ANCHOR pattern."""
    for n0 in range(PREFIX, 15, -8):
        for f in (f for f in entry.forms if f.raw is not None):
            n = min(n0, len(f.raw))
            hits = matches(img, f.raw[:n], f.fixed[:n])
            if len(hits) == 1:
                return f, hits[0]
    ida = ANCHOR.get((img.os, entry.name))
    if ida and entry.forms and entry.forms[0].raw is not None:
        toks = ida.split()
        hits = matches(img, bytes(0 if t == "??" else int(t, 16) for t in toks), [t != "??" for t in toks])
        if len(hits) == 1:
            start = max((s for s in img.fde_end if s <= hits[0]), default=None)
            if start is not None and hits[0] < img.fde_end[start]:
                return entry.forms[0], start
    return None


def update(entry, os_, imgs, accept, problems):
    for v, img in imgs.items():
        what = f"{os_} {entry.name} in {v}"
        m = locate(entry, img)
        if m is None:
            c = find_changed(entry, img)
            if c is None:
                missing = sorted(set().union(*(f.builds for f in entry.forms if f.raw is None)))
                problems.append(f"{what}: not found" + (f" (no bytes for the forms of {' '.join(missing)}: "
                                                        "add one of those builds to compare)" if missing else ""))
                continue
            close, va = c
            raw, fixed = pattern(img, va, entry.split)
            offs = carry_offsets(close.raw, close.globals, img, va, entry.split)
            if v not in accept:
                problems.append(f"{what}: changed\n      " + "\n      ".join(diff(close.raw, img, va, entry.split)))
                continue
            for o, n in zip(close.globals, offs):
                if o is not None and n is None:
                    print(f"accepted: {what}: global at {o:#x} of the closest form has no counterpart in the new "
                          f"form; not checked through this form")
            f = Form(raw, fixed, offs)
            entry.forms.append(f)
            if locate(entry, img) != (f, va):
                problems.append(f"{what}: new form doesn't identify the function uniquely")
                continue
            print(f"accepted: {what}: new form")
            m = (f, va)
        f, va = m
        f.builds.add(v)
        for i, off in enumerate(f.globals):
            if off is None:
                continue
            g = img.u32(va + off)
            n = img.refs[g]
            if n not in entry.globals[i]:
                if v in accept:
                    entry.globals[i].add(n)
                    print(f"accepted: {what}: global {g:#x} referenced {n} times")
                else:
                    problems.append(f"{what}: global {g:#x} referenced {n} times, accepted {sorted(entry.globals[i])}")
        if entry.callers is not None:
            n = img.calls[va]
            if n not in entry.callers:
                if v in accept:
                    entry.callers.add(n)
                    print(f"accepted: {what}: {n} call sites")
                else:
                    problems.append(f"{what}: {n} call sites, accepted {sorted(entry.callers)}")


def create(os_, keys, name, start, opts, imgs, problems):
    ref = imgs[REF]
    raw, fixed = pattern(ref, start, opts.get("split", False))
    if len(raw) > opts.get("max_len", MAX_LEN):
        problems.append(f"{os_} {name}: {len(raw):#x} bytes, over the limit")
        return None
    e = Entry(keys, name)
    e.split = opts.get("split", False)
    offs, seen = [], set()
    if opts.get("globals", True):
        for o, g in abs_sites(ref, start, len(raw)):
            if g not in seen and ref.writable(g) and ref.refs[g] <= BUSY:
                seen.add(g)
                offs.append(o)
    e.forms.append(Form(raw, fixed, offs))
    e.globals = [set() for _ in offs]
    e.callers = set() if opts.get("callers") else None
    print(f"new: {os_} {name}: {len(raw):#x} bytes, {len(offs)} globals" + (", call sites" if e.callers is not None else ""))
    update(e, os_, imgs, set(imgs), problems)
    return e


# deps.rs reading and writing

def nums(s):
    return [int(x, 0) for x in re.findall(r"0x[0-9A-Fa-f]+|\d+", s)]


def read_data(path):
    entries = {}
    if not os.path.exists(path):
        return entries
    os_, e, builds = None, None, set()
    for line in open(path, encoding="utf-8"):
        s = line.strip()
        if s.startswith("pub static WINDOWS"):
            os_ = "win"
        elif s.startswith("pub static LINUX"):
            os_ = "lin"
        elif s.startswith("// dep: "):
            e = Entry((), s[len("// dep: "):])
            entries[(os_, e.name)] = e
        elif s.startswith("keys: "):
            e.keys = tuple(re.findall(r'"([^"]+)"', s))
        elif s.startswith("globals: &["):
            e.globals = [set(nums(g)) for g in re.findall(r"&\[([^\[\]]*)\]", s[len("globals: &["):])]
        elif s.startswith("callers: "):
            c = nums(s)
            e.callers = set(c) if c else None
        elif s.startswith("// builds: "):
            builds = set(s[len("// builds: "):].split())
        elif s.startswith("Variant {"):
            g = [None if o == ABSENT else o for o in nums(re.search(r"globals: &\[([^\]]*)\]", s).group(1))]
            holes = nums(re.search(r"holes: &\[([^\]]*)\]", s).group(1))
            f = Form(None, None, g, builds)
            f.fingerprint = (re.search(r'prefix: "([^"]*)"', s).group(1), int(re.search(r"len: (\w+)", s).group(1), 0),
                             list(zip(holes[::2], holes[1::2])), int(re.search(r"hash: (\w+)", s).group(1), 0))
            e.forms.append(f)
    return entries


def write_data(sections):
    out = ["// Generated by tools/depsig.py: the accepted dependency state. Change it only through the tool.",
           "#![cfg_attr(rustfmt, rustfmt::skip)]", "use crate::base::deps::{Dep, Variant};", ""]
    for os_, const, cfg, entries, ref in sections:
        out += [f"#[cfg({cfg})]", f"pub static {const}: &[Dep] = &["]
        for e in entries:
            out.append(f"    // dep: {e.name}")
            m = locate(e, ref)
            if e.globals and m:
                f, va = m
                out.append(f"    // globals in {REF}: " + " ".join(f"{ref.u32(va + o):#x}" if o is not None else "-" for o in f.globals))
            out.append("    Dep {")
            out.append("        keys: &[" + ", ".join(f'"{k}"' for k in e.keys) + "],")
            out.append("        globals: &[" + ", ".join("&[" + ", ".join(map(str, sorted(g))) + "]" for g in e.globals) + "],")
            out.append("        callers: &[" + ", ".join(map(str, sorted(e.callers or ()))) + "],")
            out.append("        variants: &[")
            for f in e.forms:
                out.append("            // builds: " + " ".join(sorted(f.builds)))
                prefix, n, holes, h = f.fingerprint
                out.append(f'            Variant {{ prefix: "{prefix}", len: {n:#x}, holes: &['
                           + ", ".join(f"({o:#x}, {k})" for o, k in holes) + f"], hash: {h:#018x}, globals: &["
                           + ", ".join(f"{ABSENT if o is None else o:#x}" for o in f.globals) + "] },")
            out += ["        ],", "    },"]
        out += ["];", ""]
    return "\n".join(out)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    builds_dir = sys.argv[1]
    accept = set()
    if "--accept" in sys.argv:
        accept = set(sys.argv[sys.argv.index("--accept") + 1].split(","))
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    path = os.path.join(here, "patches", "src", "deps.rs")
    names = [(o, n) for o, _, n, _, _ in MANIFEST]
    if len(set(names)) != len(names):
        print("MANIFEST names must be unique per OS")
        return 2
    old = read_data(path)
    problems, sections = [], []
    for os_, lib, const, cfg in (("win", "byondcore.dll", "WINDOWS", "windows"),
                                 ("lin", "libbyond.so", "LINUX", 'target_os = "linux"')):
        imgs = {p.split(os.sep)[-2]: Image(os_, p) for p in sorted(glob.glob(os.path.join(builds_dir, "*", lib)))}
        if REF not in imgs:
            print(f"{builds_dir} lacks {REF}/{lib}")
            return 2
        unknown = accept - set(imgs)
        if unknown:
            print(f"--accept: no such build {sorted(unknown)}")
            return 2
        entries = []
        for (o, keys, name, start, opts) in MANIFEST:
            if o != os_:
                continue
            e = old.get((os_, name))
            if e is None:
                e = create(os_, keys, name, start, opts, imgs, problems)
            else:
                e.keys, e.split = keys, opts.get("split", False)
                err = rebuild(e, os_, imgs)
                if err:
                    print(err)
                    return 2
                update(e, os_, imgs, accept, problems)
            if e:
                entries.append(e)
        sections.append((os_, const, cfg, entries, imgs[REF]))
    if problems:
        print("\n".join(problems))
        print(f"{len(problems)} problem(s); deps.rs not written. Check the patches against these builds, "
              "then rerun with --accept <builds>.")
        return 1
    body = write_data(sections)
    if os.path.exists(path) and open(path, encoding="utf-8").read() == body:
        print("deps.rs up to date")
    else:
        open(path, "w", newline="\n", encoding="utf-8").write(body)
        print("wrote", path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
