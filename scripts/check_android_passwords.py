"""Synthetic CLI -> Android password writer -> CLI -> Android roundtrip.

Requires an installed debug app/test APK containing MdbxCliPasswordAlignmentTest.
No real vaults, credentials, cloud accounts or default ADB-device selection.
"""
import argparse
import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--serial", required=True)
    parser.add_argument("--package", default="takagi.ru.monica")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--toolchain", default="1.97.0-x86_64-pc-windows-gnu")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("choose a new output directory")
    if args.package not in {"takagi.ru.monica", "takagi.ru.monica.fdroid"}:
        parser.error("only the two Monica test applications are supported")
    adb = [args.adb, "-s", args.serial]
    root = Path(__file__).resolve().parents[1]
    cargo = ["cargo"] + (["+" + args.toolchain] if args.toolchain else [])

    def run(*command, **kwargs):
        return subprocess.run(command, check=True, timeout=900, **kwargs)

    activity = run(*adb, "shell", "dumpsys", "activity", capture_output=True).stdout
    if b"Active instrumentation" in activity or b"mActiveInstrumentation" in activity:
        parser.error("another instrumentation is active on this device")
    args.output.mkdir(parents=True)
    output = args.output.resolve()
    token = "password-alignment-" + uuid.uuid4().hex
    relative = "files/" + token
    private = "/data/user/0/" + args.package + "/" + relative
    remote_tar = "/data/local/tmp/" + token + ".tar"

    def cargo_test(name, variable, directory, log):
        with (output / log).open("wb") as stream:
            run(*cargo, "test", "--locked", "--jobs", "1", "--lib", name, "--", "--ignored",
                cwd=root, env=dict(os.environ, **{variable: str(directory)}),
                stdout=stream, stderr=subprocess.STDOUT)

    def push(directory, name):
        with tempfile.TemporaryDirectory(prefix="monica-passwords-") as temporary:
            archive = Path(temporary) / "fixture.tar"
            with tarfile.open(archive, "w", format=tarfile.USTAR_FORMAT) as tar:
                tar.add(directory, arcname=relative + "/" + name)
            run(*adb, "push", str(archive), remote_tar, capture_output=True)
            run(*adb, "shell", "run-as", args.package, "tar", "xf", remote_tar, capture_output=True)

    def instrument(name, phase, log, save=False):
        command = adb + ["shell", "am", "instrument", "-w", "-r", "-e", "class",
            "takagi.ru.monica.repository.MdbxCliPasswordAlignmentTest", "-e", "monicaPasswordInputDir",
            private + "/" + name, "-e", "monicaPasswordPhase", phase]
        if save:
            command += ["-e", "monicaPasswordOutputDir", private + "/output"]
        result = run(*command, args.package + ".test/androidx.test.runner.AndroidJUnitRunner", capture_output=True)
        (output / log).write_bytes(result.stdout + result.stderr)
        if b"OK (1 test)" not in result.stdout or b"FAILURES!!!" in result.stdout:
            raise RuntimeError("Android password contract failed: " + str(output / log))

    try:
        fixture = output / "fixture"
        cargo_test("export_android_password_fixture", "MONICA_PASSWORD_FIXTURE_DIR", fixture, "cli-fixture.log")
        push(fixture, "input")
        instrument("input", "edit", "android-edit.log", save=True)
        returned = output / "returned"
        returned.mkdir()
        data = run(*adb, "exec-out", "run-as", args.package, "tar", "cf", "-", "-C", relative + "/output", ".", capture_output=True).stdout
        with tarfile.open(fileobj=io.BytesIO(data)) as tar:
            if any(not (item.isfile() or item.isdir()) for item in tar.getmembers()):
                raise RuntimeError("unexpected non-file output")
            tar.extractall(returned, filter="data")
        shutil.copy2(fixture / "manifest.json", returned / "manifest.json")
        cargo_test("verify_android_password_return", "MONICA_PASSWORD_RETURN_DIR", returned, "cli-return.log")
        push(returned, "cli-return")
        instrument("cli-return", "verify-cli", "android-verify-cli.log")
        print("Password adapter roundtrip passed:", output)
    finally:
        subprocess.run(adb + ["shell", "run-as", args.package, "rm", "-rf", relative], check=False, capture_output=True, timeout=30)
        subprocess.run(adb + ["shell", "rm", "-f", remote_tar], check=False, capture_output=True, timeout=30)


if __name__ == "__main__":
    main()
