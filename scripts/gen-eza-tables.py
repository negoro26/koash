#!/usr/bin/env python3
"""Regenerate crates/shell-builtins/src/eza/tables.rs from an eza checkout.

Usage: scripts/gen-eza-tables.py <eza-checkout> [output]

The tables are the icon and file-type maps of eza v0.23.5 (MIT, see THIRD-PARTY.md).
"""
import os
import re
import subprocess
import sys

eza = sys.argv[1]
out_path = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "crates/shell-builtins/src/eza/tables.rs"
)


def read(rel):
    with open(os.path.join(eza, rel), encoding="utf-8") as fh:
        return fh.read()


def block(src, name):
    match = re.search(rf"const {name}: Map<[^>]*> = phf_map! \{{(.*?)\n\}};", src, re.S)
    if not match:
        sys.exit(f"{name} not found")
    return match.group(1)


def key_of(raw):
    return bytes(raw, "utf-8").decode("unicode_escape") if "\\" in raw else raw


icons_src = read("src/output/icons.rs")
glyph = {
    name: chr(int(code, 16))
    for name, code in re.findall(r"const (\w+):\s*char\s*=\s*'\\u\{([0-9a-fA-F]+)\}';", icons_src)
}


def icon_entries(name):
    entries = {}
    for line in block(icons_src, name).splitlines():
        line = line.split("//")[0].strip()
        if not line:
            continue
        match = re.match(r'"((?:[^"\\]|\\.)*)"\s*=>\s*(?:Icons::(\w+)|\'\\u\{([0-9a-fA-F]+)\}\'),?$', line)
        if not match:
            sys.exit(f"cannot parse icon line: {line!r}")
        key, const, code = match.groups()
        entries[key_of(key)] = glyph[const] if const else chr(int(code, 16))
    return entries


types_src = read("src/info/filetype.rs")


def type_entries(name):
    entries = {}
    for line in block(types_src, name).splitlines():
        line = line.split("//")[0].strip()
        if not line or line.startswith("/*"):
            continue
        match = re.match(r'"((?:[^"\\]|\\.)*)"\s*=>\s*FileType::(\w+),?$', line)
        if not match:
            sys.exit(f"cannot parse type line: {line!r}")
        entries[key_of(match.group(1))] = match.group(2)
    return entries


def rust_str(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def rust_char(ch):
    return "'\\u{%x}'" % ord(ch)


lines = ["use super::Class;", ""]
for const, source_name in (("FOLDER", "FOLDER"), ("FOLDER_OPEN", "FOLDER_OPEN"), ("FILE", "FILE"), ("FILE_UNKNOWN", "FILE_UNKNOW")):
    lines.append(f"pub(super) const {const}: char = {rust_char(glyph[source_name])};")
lines.append("")
for name in ("DIRECTORY_ICONS", "FILENAME_ICONS", "EXTENSION_ICONS"):
    entries = icon_entries(name)
    lines.append(f"pub(super) static {name}: &[(&str, char)] = &[")
    for key in sorted(entries, key=lambda k: k.encode()):
        lines.append(f"    ({rust_str(key)}, {rust_char(entries[key])}),")
    lines.append("];")
    lines.append("")
for name in ("FILENAME_TYPES", "EXTENSION_TYPES"):
    entries = type_entries(name)
    lines.append(f"pub(super) static {name}: &[(&str, Class)] = &[")
    for key in sorted(entries, key=lambda k: k.encode()):
        lines.append(f"    ({rust_str(key)}, Class::{entries[key]}),")
    lines.append("];")
    lines.append("")

source = "\n".join(lines).rstrip() + "\n"
formatted = subprocess.run(
    ["rustfmt", "--edition", "2024", "--emit", "stdout"], input=source, capture_output=True, text=True, check=True
).stdout
with open(out_path, "w", encoding="utf-8") as fh:
    fh.write(formatted)
