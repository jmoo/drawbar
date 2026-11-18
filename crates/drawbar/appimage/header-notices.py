"""Usage: header-notices.py SOURCE OUTPUT

Writes the leading comment block of every .c and .h file under SOURCE to OUTPUT,
verbatim, each distinct block once, after the files that carry it. Fails when no
block holds a copyright line.
"""

import pathlib
import re
import sys

LEADING_COMMENT = re.compile(rb"\s*(/\*.*?\*/|(?://[^\n]*\n[ \t]*)+)", re.S)

source = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])

files_by_block: dict[bytes, list[str]] = {}
for path in sorted(source.rglob("*")):
    if path.suffix not in (".c", ".h") or not path.is_file():
        continue
    comment = LEADING_COMMENT.match(path.read_bytes())
    if comment:
        block = comment.group(1).rstrip()
        files_by_block.setdefault(block, []).append(str(path.relative_to(source)))

if not any(re.search(rb"copyright", block, re.I) for block in files_by_block):
    sys.exit(f"no leading comment in {source}'s .c or .h files holds a copyright line")

with output.open("wb") as notices:
    for block, files in files_by_block.items():
        notices.write(("\n".join(files) + "\n\n").encode())
        notices.write(block + b"\n\n----\n\n")
