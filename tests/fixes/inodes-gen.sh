# Writes list/: 40 files and 2 subdirectories, so flist() reads many entries in one listing. Run by tools/fixtest.py.
mkdir -p list/sub1 list/sub2
for i in $(seq -w 1 40); do : > "list/f$i"; done
