// Only reproduces on a filesystem with 64-bit inode numbers (e.g. WSL /mnt/c): run tools/fixtest.py with TMPDIR there.
// Elsewhere stock passes too and fixtest reports "not needed".
/world/New()
	var/list/l = flist("list/")
	world.log << "[l.len] entries, [l.Find("f40") ? "f40" : "no f40"], [l.Find("sub1/") ? "sub1/" : "no sub1/"]"
	world.log << "compiled and ran"
	shutdown()
