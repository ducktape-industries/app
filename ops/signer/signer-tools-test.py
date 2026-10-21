#!/usr/bin/env python3
# Tests for the signer's app tools (developer-id.sh, release-manifest.sh): each runs in a copy of its repo over stubs of security, the signing scripts, cargo, curl and aws, which log what they saw.
# Run it with `python3 ops/signer/signer-tools-test.py`; it needs only python3, bash and tar.
import base64
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
IDENTITY = "0A1B2C3D4E5F60718293A4B5C6D7E8F901234567"
NOTARY_KEY = "-----BEGIN PRIVATE KEY-----\nnotary-secret\n-----END PRIVATE KEY-----\n"
SECRETS = {
    "notary-key": base64.b64encode(NOTARY_KEY.encode()).decode(),
    "notary-key-id": "KEYID12345",
    "notary-issuer": "issuer-0000",
    "release-signing-key": "07" * 32,
    "r2-access-key-id": "r2-access-secret",
    "r2-secret-access-key": "r2-secret-secret",
    "r2-account-id": "r2account",
    "r2-bucket": "ducktape-bucket",
}
WATCHED = ["RELEASE_SIGNING_KEY", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "DUCKTAPE_NOTARY_KEY",
           "DUCKTAPE_NOTARY_KEY_ID", "DUCKTAPE_NOTARY_ISSUER", "DUCKTAPE_CODESIGN_IDENTITY",
           "DUCKTAPE_CODESIGN_KEYCHAIN", "DUCKTAPE_PROVISIONING_PROFILE", "DUCKTAPE_ENTITLEMENTS",
           "DUCKTAPE_HELPER_ENTITLEMENTS"]

LOGGING = f"""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
def log(tool, **more):
    entry = {{"tool": tool, "argv": sys.argv[1:], "cwd": os.getcwd()}}
    entry["env"] = {{name: os.environ[name] for name in {WATCHED!r} if name in os.environ}}
    entry.update(more)
    with open(os.environ["STUB_LOG"], "a") as file:
        file.write(json.dumps(entry) + "\\n")
"""

STUBS = {
    "security": """
log("security")
args = sys.argv[1:]
if args[0] == "find-identity":
    if os.environ.get("STUB_NO_IDENTITY"):
        print("     0 valid identities found")
    else:
        print('  1) %s "Developer ID Application: Ducktape (TEAM123)"' % os.environ["STUB_IDENTITY"])
        print("     1 valid identities found")
elif args[0] == "find-certificate":
    print("SHA-256 hash: FFFF")
    print("SHA-1 hash: %s" % os.environ["STUB_IDENTITY"].lower())
    print("-----BEGIN CERTIFICATE-----")
    print("Q0VSVA==")
    print("-----END CERTIFICATE-----")
elif args[0] == "find-generic-password":
    secrets = json.loads(os.environ["STUB_SECRETS"])
    service, account = args[args.index("-s") + 1], args[args.index("-a") + 1]
    if service != "dev.ducktape.signer" or account not in secrets:
        sys.exit(44)
    print(secrets[account])
else:
    sys.exit(2)
""",
    "openssl": """
log("openssl", stdin=sys.stdin.read())
print("serial=0A1B2C")
""",
    "plutil": """
log("plutil")
print("dev.ducktape.app")
""",
    "swift": """
args = sys.argv[1:]
log("swift", notary_key=Path(os.environ["DUCKTAPE_NOTARY_KEY"]).read_text())
Path(args[3]).write_text("profile for " + args[1] + " " + args[2])
""",
    "spctl": """
log("spctl")
""",
    "tar": """
log("tar")
args = [arg for arg in sys.argv[1:] if arg not in ("--no-mac-metadata", "--no-xattrs")]
os.execv(os.environ["STUB_REAL_TAR"], ["tar", *args])
""",
    "cargo": """
log("cargo")
target = Path(os.environ.get("CARGO_TARGET_DIR") or Path.cwd() / "target") / "release"
target.mkdir(parents=True, exist_ok=True)
shutil_source = Path(os.environ["STUB_PUBLISH_RELEASE"])
(target / "publish-release").write_text(shutil_source.read_text())
(target / "publish-release").chmod(0o755)
""",
    "curl": """
log("curl")
mirrored = os.environ.get("STUB_MIRRORED")
if mirrored is None:
    sys.exit(22)
print(json.dumps({"manifest": json.dumps({"sequence": int(mirrored)}), "signature": "00"}))
""",
    "aws": """
log("aws")
""",
}

PUBLISH_RELEASE = """
import hashlib
args = sys.argv[1:]
flags = {}
archives = []
for flag, value in zip(args[::2], args[1::2]):
    if flag == "--archive":
        archives.append(value.split("=", 1))
    else:
        flags[flag] = value
log("publish-release", inputs=os.environ.get("SIGNER_INPUTS"))
out = Path(flags["--out"])
out.mkdir(parents=True, exist_ok=True)
channel = flags["--channel"]
files, names, artifacts = [], [], {}
for platform, path in archives:
    data = Path(path).read_bytes()
    sha = hashlib.sha256(data).hexdigest()
    artifacts[platform] = {"sha256": sha, "size": len(data)}
    name = f"Release-{platform}-{sha}.tar.gz"
    (out / name).write_bytes(data)
    names.append(name)
    files.append({"path": f"{flags['--record'].rstrip('/')}/{sha}/{name}", "from": f"{out.name}/{name}"})
manifest = {"schema": 1, "channel": channel, "sequence": int(flags["--sequence"]), "artifacts": artifacts}
(out / f"{channel}.json").write_text(json.dumps({"manifest": json.dumps(manifest), "signature": "00" * 64}))
(out / "record.json").write_text("record of " + flags["--sequence"])
tag = {"name": f"release/{channel}/{flags['--sequence']}", "message": "record", "time": int(flags["--sequence"])}
(out / "publish.json").write_text(json.dumps({"files": files, "tag": tag}))
for name in names + [f"{channel}.json"]:
    print(name)
"""


class Rig:
    def __init__(self, root):
        self.root = root.resolve()
        root = self.root
        self.repo = root / "repo"
        (self.repo / "ops" / "signer").mkdir(parents=True)
        for tool in ("developer-id.sh", "release-manifest.sh", "r2-mirror.sh"):
            shutil.copy(HERE / tool, self.repo / "ops" / "signer" / tool)
            (self.repo / "ops" / "signer" / tool).chmod(0o755)
        sign = self.repo / "ops" / "sign-app-macos.sh"
        sign.write_text(LOGGING + "\nlog('sign-app-macos', profile=Path(os.environ['DUCKTAPE_PROVISIONING_PROFILE']).read_text())\n"
                        "Path(sys.argv[2]).write_text('dmg')\n(Path(sys.argv[1]) / 'Contents' / 'signed').write_text('yes')\n")
        sign.chmod(0o755)
        (self.repo / "ops" / "apple-profile.swift").write_text("// stub\n")
        self.bin = root / "bin"
        self.bin.mkdir()
        for name, body in STUBS.items():
            path = self.bin / name
            path.write_text(LOGGING + body)
            path.chmod(0o755)
        self.publish_release = root / "publish-release"
        self.publish_release.write_text(LOGGING + PUBLISH_RELEASE)
        self.log = root / "log"
        self.work = root / "work"
        self.inputs = self.work / "inputs"
        self.out = self.work / "out"
        self.out.mkdir(parents=True)

    def env(self, **more):
        env = {
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "HOME": str(self.root),
            "TMPDIR": str(self.root),
            "STUB_LOG": str(self.log),
            "STUB_IDENTITY": IDENTITY,
            "STUB_SECRETS": json.dumps(SECRETS),
            "STUB_REAL_TAR": shutil.which("tar"),
            "STUB_PUBLISH_RELEASE": str(self.publish_release),
            "SIGNER_REPO": "app",
            "SIGNER_COMMIT": "c0ffee" * 6 + "c0ff",
            "SIGNER_CHECK": "1900",
            "SIGNER_INPUTS": str(self.work / "inputs.json"),
        }
        env.update(more)
        return env

    def run(self, tool, **env):
        return subprocess.run(["bash", str(self.repo / "ops" / "signer" / tool), str(self.inputs), str(self.out)],
                              capture_output=True, text=True, env=self.env(**env))

    def entries(self, tool=None):
        if not self.log.exists():
            return []
        entries = [json.loads(line) for line in self.log.read_text().splitlines()]
        return [entry for entry in entries if tool is None or entry["tool"] == tool]

    def unsigned_bundle(self):
        macos = self.inputs / "macos"
        macos.mkdir(parents=True)
        app = self.root / "bundle" / "Ducktape.app" / "Contents"
        app.mkdir(parents=True)
        (app / "Info.plist").write_text("<plist/>")
        with tarfile.open(macos / "ducktape-app-unsigned.app.tar.gz", "w:gz") as archive:
            archive.add(app.parent, arcname="Ducktape.app")
        (macos / "entitlements.plist").write_text("built commit's entitlements")
        (macos / "helper-entitlements.plist").write_text("built commit's helper entitlements")

    def release_inputs(self, sequence=1792500000):
        linux = self.inputs / "linux"
        linux.mkdir(parents=True)
        plan = {"commit": "c0ffee" * 6 + "c0ff", "sequence": sequence, "version": "0.1.0",
                "display": "0.1.0+c0ffeec", "published_at": "2026-10-20T07:20:00Z"}
        (linux / "plan.json").write_text(json.dumps(plan))
        (linux / "ducktape-app-x86_64-unknown-linux-gnu.tar.gz").write_bytes(b"linux archive")
        signed = self.inputs / "macos-sign"
        signed.mkdir(parents=True)
        (signed / "ducktape-app-aarch64-apple-darwin.app.tar.gz").write_bytes(b"macos archive")

    def published(self, channel, sequence=1792500000):
        self.release_inputs(sequence)
        ran = self.run("release-manifest.sh", SIGNER_ENV_CHANNEL=channel)
        assert ran.returncode == 0, said(ran)
        shutil.move(self.out, self.inputs / "publish")
        self.out.mkdir()
        self.log.unlink()
        return (self.inputs / "publish" / "upload-order").read_text().split()


def said(ran):
    return ran.stdout + ran.stderr


class DeveloperId(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.rig = Rig(Path(self.temp.name))
        self.rig.unsigned_bundle()

    def tearDown(self):
        self.temp.cleanup()

    def test_it_signs_with_the_built_commits_entitlements_and_the_identitys_profile(self):
        ran = self.rig.run("developer-id.sh")
        self.assertEqual(ran.returncode, 0, said(ran))
        [sign] = self.rig.entries("sign-app-macos")
        macos = self.rig.inputs / "macos"
        self.assertEqual(sign["env"]["DUCKTAPE_ENTITLEMENTS"], str(macos / "entitlements.plist"))
        self.assertEqual(sign["env"]["DUCKTAPE_HELPER_ENTITLEMENTS"], str(macos / "helper-entitlements.plist"))
        self.assertEqual(sign["env"]["DUCKTAPE_CODESIGN_IDENTITY"], IDENTITY)
        self.assertEqual(sign["profile"], "profile for dev.ducktape.app 0A1B2C")
        self.assertEqual(sign["argv"][1], str(self.rig.out / "ducktape-app-aarch64-apple-darwin.dmg"))
        self.assertEqual(sign["env"]["DUCKTAPE_NOTARY_KEY_ID"], SECRETS["notary-key-id"])
        [swift] = self.rig.entries("swift")
        self.assertEqual(swift["notary_key"], NOTARY_KEY, "the .p8 as it was before base64")
        self.assertFalse(Path(sign["env"]["DUCKTAPE_NOTARY_KEY"]).exists(), "the notary key's file is gone after")
        assessed = [entry["argv"][-1] for entry in self.rig.entries("spctl")]
        self.assertEqual(assessed, [sign["argv"][1], sign["argv"][0]], "the DMG, then the app")
        self.assertEqual(sorted(path.name for path in self.rig.out.iterdir()),
                         ["ducktape-app-aarch64-apple-darwin.app.tar.gz", "ducktape-app-aarch64-apple-darwin.dmg"])
        with tarfile.open(self.rig.out / "ducktape-app-aarch64-apple-darwin.app.tar.gz") as archive:
            self.assertIn("Ducktape.app/Contents/signed", archive.getnames(), "the archive holds the signed bundle")
        for value in [*SECRETS.values(), "notary-secret"]:
            self.assertNotIn(value, said(ran))

    def test_without_an_identity_it_signs_nothing(self):
        ran = self.rig.run("developer-id.sh", STUB_NO_IDENTITY="1")
        self.assertNotEqual(ran.returncode, 0)
        self.assertIn("no Developer ID Application identity", ran.stderr)
        self.assertEqual(self.rig.entries("sign-app-macos"), [])
        self.assertEqual(list(self.rig.out.iterdir()), [])


class ReleaseManifest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.rig = Rig(Path(self.temp.name))
        self.rig.release_inputs()

    def tearDown(self):
        self.temp.cleanup()

    def test_it_builds_before_any_key_signs_and_records_and_touches_no_mirror(self):
        ran = self.rig.run("release-manifest.sh", SIGNER_ENV_CHANNEL="nightly")
        self.assertEqual(ran.returncode, 0, said(ran))
        tools = [entry["tool"] for entry in self.rig.entries()]
        self.assertNotIn("aws", tools, "a publish the signer may still refuse never reaches R2")
        self.assertNotIn("curl", tools)
        asked_for = [entry["argv"][entry["argv"].index("-a") + 1] for entry in self.rig.entries("security")]
        self.assertEqual(asked_for, ["release-signing-key"], "no R2 key is read")
        self.assertLess(tools.index("cargo"), tools.index("security"), "publish-release is built before a key is read")
        [cargo] = self.rig.entries("cargo")
        self.assertEqual(cargo["argv"], ["build", "--locked", "--release", "-p", "ducktape-updater", "--bin", "publish-release"])
        self.assertEqual(cargo["cwd"], str(self.rig.repo))
        self.assertEqual(cargo["env"], {})
        [publish] = self.rig.entries("publish-release")
        self.assertEqual(publish["env"], {"RELEASE_SIGNING_KEY": SECRETS["release-signing-key"]})
        argv = publish["argv"]
        flags = dict(zip(argv[::2], argv[1::2]))
        self.assertEqual((flags["--channel"], flags["--sequence"], flags["--display"], flags["--published-at"]),
                         ("nightly", "1792500000", "0.1.0+c0ffeec", "2026-10-20T07:20:00Z"))
        self.assertEqual(flags["--record"], "/shared/releases/app/")
        archives = [value for flag, value in zip(argv[::2], argv[1::2]) if flag == "--archive"]
        self.assertEqual(archives, [
            f"linux-x86_64={self.rig.inputs}/linux/ducktape-app-x86_64-unknown-linux-gnu.tar.gz",
            f"macos-aarch64={self.rig.inputs}/macos-sign/ducktape-app-aarch64-apple-darwin.app.tar.gz",
        ])
        self.assertEqual(sorted(path.name for path in self.rig.out.iterdir()),
                         ["nightly.json", "publish.json", "record.json", "upload-order"])
        order = (self.rig.out / "upload-order").read_text().split()
        self.assertEqual(order[-1], "nightly.json")
        self.assertEqual(len(order), 3, "publish-release's printed order, kept for r2-mirror")
        asked = json.loads((self.rig.out / "publish.json").read_text())
        for item in asked["files"]:
            source = (self.rig.work / item["from"]).resolve()
            self.assertTrue(source.is_file(), f"{item['from']} is a file of the check's work")
            self.assertIn(self.rig.work.resolve(), source.parents)
            self.assertNotIn(self.rig.out.resolve(), source.parents, "the archives are not put again as outputs")
        for value in SECRETS.values():
            self.assertNotIn(value, said(ran))

    def test_without_a_channel_it_builds_and_signs_nothing(self):
        ran = self.rig.run("release-manifest.sh")
        self.assertNotEqual(ran.returncode, 0)
        self.assertIn("CHANNEL", ran.stderr)
        self.assertEqual(self.rig.entries(), [])


class Mirror(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.rig = Rig(Path(self.temp.name))

    def tearDown(self):
        self.temp.cleanup()

    def test_it_uploads_by_publish_releases_names_archives_first_then_the_channel_file(self):
        order = self.rig.published("nightly")
        ran = self.rig.run("r2-mirror.sh", SIGNER_ENV_CHANNEL="nightly")
        self.assertEqual(ran.returncode, 0, said(ran))
        uploads = self.rig.entries("aws")
        names = [Path(entry["argv"][2]).name for entry in uploads]
        self.assertEqual(names, order, "the names and order publish-release printed, the channel file last")
        for entry, platform in zip(uploads, ["linux", "macos-sign"]):
            handed = next((self.rig.inputs / platform).glob("*.tar.gz")).read_bytes()
            self.assertEqual(Path(entry["argv"][2]).read_bytes(), handed)
        sent = Path(uploads[2]["argv"][2]).read_text()
        self.assertEqual(sent, (self.rig.inputs / "publish" / "nightly.json").read_text(), "the signed channel file, byte for byte")
        for entry in uploads:
            argv = entry["argv"]
            cache = argv[argv.index("--cache-control") + 1]
            immutable = "public, max-age=31536000, immutable"
            self.assertEqual(cache, immutable if argv[2].endswith(".tar.gz") else "public, max-age=60")
            self.assertEqual(argv[3], f"s3://ducktape-bucket/app/{Path(argv[2]).name}")
            self.assertEqual(argv[argv.index("--endpoint-url") + 1], "https://r2account.r2.cloudflarestorage.com")
            self.assertEqual(entry["env"], {"AWS_ACCESS_KEY_ID": SECRETS["r2-access-key-id"],
                                            "AWS_SECRET_ACCESS_KEY": SECRETS["r2-secret-access-key"]})
        for value in SECRETS.values():
            self.assertNotIn(value, said(ran))

    def test_it_mirrors_nothing_the_mirror_already_publishes(self):
        self.rig.published("stable")
        ran = self.rig.run("r2-mirror.sh", SIGNER_ENV_CHANNEL="stable", STUB_MIRRORED="1792500000")
        self.assertEqual(ran.returncode, 0, said(ran))
        self.assertIn("not mirrored", ran.stdout)
        self.assertEqual(self.rig.entries("aws"), [])
        self.assertEqual(self.rig.entries("security"), [], "no R2 key is read for a skipped mirror")

    def test_it_mirrors_over_an_older_published_sequence(self):
        self.rig.published("stable")
        ran = self.rig.run("r2-mirror.sh", SIGNER_ENV_CHANNEL="stable", STUB_MIRRORED="1792499999")
        self.assertEqual(ran.returncode, 0, said(ran))
        self.assertEqual(len(self.rig.entries("aws")), 3)

    def test_a_channel_file_naming_other_archives_mirrors_nothing(self):
        self.rig.published("stable")
        (self.rig.inputs / "linux" / "ducktape-app-x86_64-unknown-linux-gnu.tar.gz").write_bytes(b"another linux archive")
        ran = self.rig.run("r2-mirror.sh", SIGNER_ENV_CHANNEL="stable")
        self.assertNotEqual(ran.returncode, 0)
        self.assertIn("linux-x86_64 archive is not the one handed over", ran.stderr)
        self.assertEqual(self.rig.entries("aws"), [])
        self.assertEqual(self.rig.entries("security"), [])

    def test_without_a_channel_it_mirrors_nothing(self):
        self.rig.published("stable")
        ran = self.rig.run("r2-mirror.sh")
        self.assertNotEqual(ran.returncode, 0)
        self.assertIn("CHANNEL", ran.stderr)
        self.assertEqual(self.rig.entries(), [])


if __name__ == "__main__":
    unittest.main()
