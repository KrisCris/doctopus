#!/usr/bin/env python3
"""Check that a release binary runs on a bare system.

Usage: check-binary.py [--glibc] BINARY

Reads the executable's own headers (ELF, Mach-O or PE) with the standard
library only, so it works the same on every CI runner:

* Linux (ELF): fully static. No program interpreter, no shared libraries.
  With --glibc, for the builds that link glibc dynamically: only glibc's
  loader and the libraries every glibc system has (libc, libm, libgcc_s,
  libpthread, libdl, librt, libutil).
* macOS (Mach-O): only libraries every Mac has (/usr/lib, /System/Library).
  Apple does not support static executables.
* Windows (PE): no C runtime DLLs (vcruntime, msvcp, ucrtbase,
  api-ms-win-crt-*, MinGW runtimes). System DLLs such as kernel32 are fine.

Exits 1 and says why when a check fails.
"""

import re
import struct
import sys

# With --glibc: glibc's loader (ld-linux-x86-64.so.2, ld-linux-aarch64.so.1),
# its libraries, and libgcc_s, which every glibc system has.
GLIBC_LOADER = re.compile(r"^ld-linux[-\w]*\.so\.\d+$")
GLIBC_LIBRARIES = {
    "libc.so.6",
    "libm.so.6",
    "libgcc_s.so.1",
    "libpthread.so.0",
    "libdl.so.2",
    "librt.so.1",
    "libutil.so.1",
}
MACHO_SYSTEM_PREFIXES = ("/usr/lib/", "/System/Library/")
PE_RUNTIME_DLLS = re.compile(
    r"^(vcruntime|msvcp|msvcr|ucrtbase|api-ms-win-crt-|libgcc|libstdc\+\+|libwinpthread)",
    re.IGNORECASE,
)


def c_string(data, offset):
    end = data.find(b"\0", offset)
    return data[offset : end if end >= 0 else len(data)].decode("utf-8", "replace")


# ----- ELF -------------------------------------------------------------------

PT_LOAD, PT_DYNAMIC, PT_INTERP = 1, 2, 3
DT_NULL, DT_NEEDED, DT_STRTAB = 0, 1, 5


def check_elf(data, glibc):
    is64 = data[4] == 2
    order = "<" if data[5] == 1 else ">"
    if is64:
        phoff, = struct.unpack_from(order + "Q", data, 0x20)
        phentsize, phnum = struct.unpack_from(order + "HH", data, 0x36)
    else:
        phoff, = struct.unpack_from(order + "I", data, 0x1C)
        phentsize, phnum = struct.unpack_from(order + "HH", data, 0x2A)

    segments = []
    for i in range(phnum):
        at = phoff + i * phentsize
        if is64:
            kind, _, offset, vaddr, _, filesz = struct.unpack_from(order + "IIQQQQ", data, at)
        else:
            kind, offset, vaddr, _, filesz = struct.unpack_from(order + "IIIII", data, at)
        segments.append((kind, offset, vaddr, filesz))

    problems = []
    for kind, offset, _, filesz in segments:
        if kind == PT_INTERP:
            interpreter = c_string(data, offset)
            if not (glibc and GLIBC_LOADER.match(interpreter.rsplit("/", 1)[-1])):
                problems.append("needs the program interpreter " + interpreter)

    def file_offset(address):
        for kind, offset, vaddr, filesz in segments:
            if kind == PT_LOAD and vaddr <= address < vaddr + filesz:
                return offset + address - vaddr
        return None

    libraries = []
    for kind, offset, _, filesz in segments:
        if kind != PT_DYNAMIC:
            continue
        entry = "qQ" if is64 else "iI"
        size = struct.calcsize(order + entry)
        entries = [
            struct.unpack_from(order + entry, data, offset + i * size)
            for i in range(filesz // size)
        ]
        strtab = next((value for tag, value in entries if tag == DT_STRTAB), None)
        strtab = file_offset(strtab) if strtab is not None else None
        for tag, value in entries:
            if tag == DT_NULL:
                break
            if tag == DT_NEEDED:
                name = c_string(data, strtab + value) if strtab is not None else "?"
                libraries.append(name)
                if not (glibc and (name in GLIBC_LIBRARIES or GLIBC_LOADER.match(name))):
                    problems.append("needs the shared library " + name)
    if libraries:
        return "ELF (needs: " + ", ".join(libraries) + ")", problems
    return "ELF", problems


# ----- Mach-O ----------------------------------------------------------------

LOAD_DYLIB_COMMANDS = {0xC, 0x20, 0x80000018, 0x8000001F, 0x80000023}


def check_fat(data):
    """A universal binary: check every architecture it holds."""
    count, = struct.unpack_from(">I", data, 4)
    problems = []
    for i in range(count):
        _, _, offset, size, _ = struct.unpack_from(">iiIII", data, 8 + i * 20)
        _, found = check_macho(data[offset : offset + size])
        problems.extend(p for p in found if p not in problems)
    return f"universal Mach-O ({count} architectures)", problems


def check_macho(data):
    magic, = struct.unpack_from("<I", data, 0)
    is64 = magic == 0xFEEDFACF
    ncmds, = struct.unpack_from("<I", data, 16)
    at = 32 if is64 else 28
    problems = []
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<II", data, at)
        if cmd in LOAD_DYLIB_COMMANDS:
            name_offset, = struct.unpack_from("<I", data, at + 8)
            name = c_string(data, at + name_offset)
            if not name.startswith(MACHO_SYSTEM_PREFIXES):
                problems.append("needs the non-system library " + name)
        at += cmdsize
    return "Mach-O", problems


# ----- PE --------------------------------------------------------------------

IMPORT_DIRECTORY, DELAY_IMPORT_DIRECTORY = 1, 13


def check_pe(data):
    pe, = struct.unpack_from("<I", data, 0x3C)
    if data[pe : pe + 4] != b"PE\0\0":
        raise ValueError("not a PE file")
    sections_count, = struct.unpack_from("<H", data, pe + 6)
    optional_size, = struct.unpack_from("<H", data, pe + 20)
    optional = pe + 24
    magic, = struct.unpack_from("<H", data, optional)
    directories = optional + (112 if magic == 0x20B else 96)
    sections = [
        struct.unpack_from("<8sIIII", data, optional + optional_size + i * 40)
        for i in range(sections_count)
    ]

    def file_offset(rva):
        for _, vsize, vaddr, rawsize, rawptr in sections:
            if vaddr <= rva < vaddr + max(vsize, rawsize):
                return rawptr + rva - vaddr
        return None

    dlls = []
    for directory, name_field in ((IMPORT_DIRECTORY, 12), (DELAY_IMPORT_DIRECTORY, 4)):
        rva, size = struct.unpack_from("<II", data, directories + directory * 8)
        at = file_offset(rva) if rva and size else None
        if at is None:
            continue
        stride = 20 if directory == IMPORT_DIRECTORY else 32
        while at + stride <= len(data):
            descriptor = data[at : at + stride]
            if descriptor == b"\0" * stride:
                break
            name_rva, = struct.unpack_from("<I", data, at + name_field)
            name_at = file_offset(name_rva)
            if name_at is not None:
                dlls.append(c_string(data, name_at))
            at += stride
    problems = [
        "needs the C runtime library " + dll for dll in dlls if PE_RUNTIME_DLLS.match(dll)
    ]
    return "PE (imports: " + (", ".join(dlls) or "none") + ")", problems


def main():
    args = sys.argv[1:]
    glibc = args[:1] == ["--glibc"]
    if glibc:
        args = args[1:]
    if len(args) != 1:
        sys.exit(__doc__.strip())
    path = args[0]
    with open(path, "rb") as f:
        data = f.read()
    if data[:4] == b"\x7fELF":
        kind, problems = check_elf(data, glibc)
    elif data[:4] in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe"):
        kind, problems = check_macho(data)
    elif data[:4] == b"\xca\xfe\xba\xbe":
        kind, problems = check_fat(data)
    elif data[:2] == b"MZ":
        kind, problems = check_pe(data)
    else:
        sys.exit(f"{path}: not an ELF, Mach-O or PE executable")
    for problem in problems:
        print(f"{path}: {problem}", file=sys.stderr)
    if problems:
        sys.exit(1)
    print(f"{path}: {kind}: self-contained")


if __name__ == "__main__":
    main()
