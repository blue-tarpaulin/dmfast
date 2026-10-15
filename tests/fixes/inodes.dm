// Only reproduces on a filesystem with 64-bit inode numbers (e.g. WSL /mnt/c): run tools/fixtest.sh with TMPDIR there.
// Elsewhere stock passes too and fixtest reports "not needed".
/world/New()
	world.log << "compiled and ran"
	shutdown()
