from __future__ import annotations

import hashlib
import io
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.release = self.root / "releases" / "download" / "v0.1.0"
        self.release.mkdir(parents=True)
        self.bin = self.root / "bin"
        self.claude = self.root / "claude"
        self.claw = self.root / "claw"
        self.env = dict(
            os.environ,
            CC2CC_VERSION="v0.1.0",
            CC2CC_RELEASE_BASE_URL=(self.root / "releases").as_uri(),
            CC2CC_INSTALL_DIR=str(self.bin),
            CLAUDE_CONFIG_DIR=str(self.claude),
            CLAWCODEX_CONFIG_DIR=str(self.claw),
        )
        self.make_archives()

    def tearDown(self):
        self.temporary.cleanup()

    def make_archives(self, extra=None):
        sums = []
        for target in ("aarch64-apple-darwin", "x86_64-unknown-linux-musl"):
            asset = f"cc2cc-{target}.tar.gz"
            path = self.release / asset
            with tarfile.open(path, "w:gz") as archive:
                files = {
                    "cc2cc": b'#!/bin/sh\nprintf "cc2cc 0.1.0\\n"\n',
                    "LICENSE": (ROOT / "LICENSE").read_bytes(),
                    "skills/cc2cc/SKILL.md": (
                        ROOT / "skills/cc2cc/SKILL.md"
                    ).read_bytes(),
                }
                if extra:
                    files.update(extra)
                for name, data in files.items():
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    info.mode = 0o755 if name == "cc2cc" else 0o644
                    archive.addfile(info, io.BytesIO(data))
            sums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {asset}\n")
        (self.release / "SHA256SUMS").write_text("".join(sums))

    def run_installer(self, success=True):
        result = subprocess.run(
            ["sh", str(ROOT / "install.sh")],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=15,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def test_installs_binary_and_both_skills_and_updates(self):
        self.run_installer()
        self.run_installer()
        result = subprocess.run(
            [str(self.bin / "cc2cc"), "--version"],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertEqual(result.stdout.strip(), "cc2cc 0.1.0")
        for config in (self.claude, self.claw):
            skill = config / "skills" / "cc2cc"
            self.assertEqual(
                (skill / "SKILL.md").read_bytes(),
                (ROOT / "skills/cc2cc/SKILL.md").read_bytes(),
            )
            self.assertEqual(
                (skill / ".cc2cc-managed").read_text().strip(), "sudy-super/cc2cc"
            )

    def test_unmanaged_skill_is_kept_and_install_stops(self):
        skill = self.claw / "skills" / "cc2cc"
        skill.mkdir(parents=True)
        (skill / "SKILL.md").write_text("custom user skill")
        self.run_installer(False)
        self.assertEqual((skill / "SKILL.md").read_text(), "custom user skill")
        self.assertFalse(self.bin.exists())
        self.assertFalse(self.claude.exists())

    def test_checksum_mismatch_does_not_install(self):
        (self.release / "SHA256SUMS").write_text(
            (self.release / "SHA256SUMS").read_text().replace("  cc2cc", "BAD  cc2cc")
        )
        self.run_installer(False)
        self.assertFalse(self.bin.exists())
        self.assertFalse(self.claude.exists())

    def test_escaping_archive_path_is_rejected(self):
        self.make_archives({"../escaped": b"bad"})
        self.run_installer(False)
        self.assertFalse(self.bin.exists())
        self.assertFalse((self.root / "escaped").exists())

    def test_symlink_skill_is_kept(self):
        custom = self.root / "custom"
        custom.mkdir()
        (self.claude / "skills").mkdir(parents=True)
        (self.claude / "skills" / "cc2cc").symlink_to(custom, target_is_directory=True)
        self.run_installer(False)
        self.assertTrue((self.claude / "skills" / "cc2cc").is_symlink())
        self.assertFalse(self.bin.exists())

    def test_unsupported_platform_is_rejected(self):
        stub = self.root / "stub"
        stub.mkdir()
        uname = stub / "uname"
        uname.write_text(
            '#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo aarch64;; esac\n'
        )
        uname.chmod(0o755)
        self.env["PATH"] = str(stub) + os.pathsep + self.env["PATH"]
        self.run_installer(False)
        self.assertFalse(self.bin.exists())
