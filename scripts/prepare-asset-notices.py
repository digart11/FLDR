"""Collect explicit font/icon/binary resource notices; preserve asset versions."""
import hashlib
import json
import re
import urllib.request
from pathlib import Path

from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parents[1]
DIRECTORY = ROOT / "legal/resources"
DIRECTORY.mkdir(parents=True, exist_ok=True)


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent":"FLDR-license-audit/1.0"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read().decode("utf-8-sig")


def main():
    assets = []
    def asset(name, version, license, source, scope, texts, files=None, review=""):
        paths = []
        for filename, content in texts.items():
            path = DIRECTORY / filename
            path.write_text(content.strip()+"\n", encoding="utf-8")
            paths.append(path.relative_to(ROOT).as_posix())
        entry = {"name":name, "version":version, "license":license, "source":source, "scope":scope, "notices":paths}
        if files:
            entry["files"] = [{"path":p.relative_to(ROOT).as_posix(), "sha256":hashlib.sha256(p.read_bytes()).hexdigest()} for p in files]
        if review:
            entry["review"] = review
        assets.append(entry)
    ofl_url = "https://raw.githubusercontent.com/vercel/geist-font/main/OFL.txt"
    ofl = fetch(ofl_url)
    # Attach each font's actual copyright/RFN header to the unchanged OFL body.
    body = ofl[ofl.index("SIL OPEN FONT LICENSE Version 1.1"):]
    fonts = list((ROOT / "crates/atlas-ui/assets/fonts").rglob("*.ttf"))
    for path in fonts:
        with TTFont(path) as font:
            name = font["name"].getDebugName(1) or path.stem
            version = font["name"].getDebugName(5) or "Bundled snapshot"
            copyright = font["name"].getDebugName(0) or "See font metadata"
        scope = "desktop-font"
        asset(name + " (" + path.name + ")", version, "OFL-1.1", ofl_url, scope,
              {path.stem+"-OFL.txt": copyright+"\n\n"+body}, [path])
    phosphor_url = "https://raw.githubusercontent.com/phosphor-icons/core/main/LICENSE"
    asset("Phosphor Icons", "Bundled SVG snapshot", "MIT", phosphor_url, "desktop-artwork",
          {"Phosphor-LICENSE.txt":fetch(phosphor_url)}, list((ROOT/"crates/atlas-ui/assets/icons/ph").glob("*.svg")))
    lucide = DIRECTORY/"Lucide-LICENSE.txt"
    asset("Lucide icon artwork", "Bundled GPUI Component SVG snapshot", "ISC", "https://github.com/lucide-icons/lucide", "desktop-component-artwork",
          {"Lucide-LICENSE.txt":lucide.read_text(encoding="utf-8")}, list((ROOT/"vendor/gpui-component-fork/crates/assets/assets/icons").glob("*.svg")))
    asset("UnRAR for Windows", "7.20 x64", "LicenseRef-UnRAR", "https://www.rarlab.com/rar_add.htm", "windows-bundled-tool",
          {"UnRAR-NOTICE.txt":"UnRAR 7.20 x64 freeware\nCopyright (c) 1993-2026 Alexander Roshal\nThe unmodified UnRAR command-line executable is distributed for archive reading/extraction.\nPublisher: RARLAB — https://www.rarlab.com/\nUnRAR components and licensing: https://www.rarlab.com/rar_add.htm\nRAR/WinRAR EULA (including the UnRAR distribution exception): https://www.rarlab.com/license.htm\nDo not apply Atlas's software license to this executable."}, [ROOT/"crates/atlas-core/vendor/unrar-bin/UnRAR.exe"])
    asset("rclone", "1.74.4", "MIT", "https://github.com/rclone/rclone/tree/v1.74.4", "optional-cloud-tool",
          {"rclone-COPYING.txt":fetch("https://raw.githubusercontent.com/rclone/rclone/v1.74.4/COPYING")})
    asset("WinFsp", "2.1", "GPL-3.0-only WITH LicenseRef-WinFsp-FLOSS-exception", "https://github.com/winfsp/winfsp/tree/v2.1", "separately-installed-optional-driver",
          {"WinFsp-LICENSE.txt":fetch("https://raw.githubusercontent.com/winfsp/winfsp/v2.1/License.txt")}, review="Driver installed separately. The Rust bindings have their own GPL license. A closed-source distribution cannot assume the FLOSS exception applies.")
    (ROOT/"legal/assets.json").write_text(json.dumps(assets, indent=2, ensure_ascii=False)+"\n", encoding="utf-8")
    print(f"Prepared notices and hashes for {len(assets)} resource groups.")


if __name__ == "__main__":
    main()
