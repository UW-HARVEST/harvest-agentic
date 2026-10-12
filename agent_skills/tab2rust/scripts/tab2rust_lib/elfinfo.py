"""ELF facts of a binary from one readelf run: the file type, the sections, and the words that the
dynamic loader fills. gdb_extract.py also imports this module, so it must not import gdb or other
tab2rust modules."""
import re
import subprocess

# [Nr] Name Type Address Off Size ES Flg Lk Inf Al
SECTION_RE = re.compile(r'^\s*\[\s*\d+\]\s+(\S+)\s+(\w+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)\s+[0-9a-f]+\s+([A-Za-z]*)\s+\d',
                        re.M)
# the segment that the loader makes read-only after the relocations (.data.rel.ro and similar)
RELRO_RE = re.compile(r'^\s*GNU_RELRO\s+0x[0-9a-f]+\s+0x([0-9a-f]+)\s+0x[0-9a-f]+\s+0x[0-9a-f]+\s+0x([0-9a-f]+)', re.M)
# Offset Info Type Symbol-value Symbol-name + Addend: a word that gets the address of a symbol
RELOC_RE = re.compile(r'^\s*([0-9a-f]+)\s+[0-9a-f]+\s+R_\w+\s+[0-9a-f]+\s+([^\s@]+)\S*\s*([+-])\s*([0-9a-f]+)', re.M)
# Num: Value Size Type Bind Vis Ndx Name: a data object that the binary exports by name
DYNSYM_RE = re.compile(r'^\s*\d+:\s+[0-9a-f]+\s+\S+\s+OBJECT\s+(?:GLOBAL|WEAK)\s+(?:DEFAULT|PROTECTED)\s+\d+\s+([^\s@]+)',
                       re.M)
# Offset Info Type Addend: a word that gets an address in this file (load address + addend)
RELATIVE_RE = re.compile(r'^\s*([0-9a-f]+)\s+[0-9a-f]+\s+R_\w+_RELATIVE\s+([0-9a-f]+)\s*$', re.M)


def file_type(path):
    """The ELF file type from the header (REL, EXEC, DYN), or None if path is not an ELF file."""
    try:
        with open(path, 'rb') as f:
            head = f.read(18)
    except OSError:
        return None
    if head[:4] != b'\x7fELF':
        return None
    return {1: 'REL', 2: 'EXEC', 3: 'DYN'}.get(int.from_bytes(head[16:18], 'little'))


def read(path):
    """{type, sections: {name: {nobits, writable}}, loader: {address: [symbol or None, addend]}, exports}. type
    is the ELF file type (DYN, EXEC, REL), or None if path is not an ELF file. writable: the program
    can change the section at run time (the W flag, and not in the GNU_RELRO segment). loader: the
    words that the dynamic loader writes. Each one holds an address: symbol + addend, or (symbol None)
    the address addend in this file. The file does not always have the value of such a word.
    exports: the names of the data objects in the dynamic symbol table (other programs link to them)."""
    kind = file_type(path)
    if kind is None:
        return {'type': None, 'sections': {}, 'loader': {}, 'exports': set()}
    out = subprocess.run(['readelf', '-lSrW', '--dyn-syms', path], capture_output=True, text=True).stdout
    relro = [(int(a, 16), int(a, 16) + int(n, 16)) for a, n in RELRO_RE.findall(out)]
    sections = {name: {'nobits': kind == 'NOBITS',
                       'writable': 'W' in flags and not any(lo <= int(a, 16) < hi for lo, hi in relro)}
                for name, kind, a, _, flags in SECTION_RE.findall(out)}
    loader = {int(off, 16): [None, int(add, 16)] for off, add in RELATIVE_RE.findall(out)}
    loader.update({int(off, 16): [sym, int(add, 16) * (1 if sign == '+' else -1)]
                   for off, sym, sign, add in RELOC_RE.findall(out)})
    return {'type': kind, 'sections': sections, 'loader': loader, 'exports': set(DYNSYM_RE.findall(out))}
