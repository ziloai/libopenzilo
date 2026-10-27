#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Build native code and Kotlin bindings. Requires Rust + an installed Android NDK."""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
ABIS = {"arm64-v8a": ("aarch64-linux-android", "aarch64-linux-android"),
        "x86_64": ("x86_64-linux-android", "x86_64-linux-android"),
        "armeabi-v7a": ("armv7-linux-androideabi", "armv7a-linux-androideabi")}

def run(*args, env=None):
    subprocess.run([str(x) for x in args], cwd=ROOT, env=env, check=True)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk", type=Path, default=os.environ.get("ANDROID_NDK_HOME"))
    parser.add_argument("--abi", choices=list(ABIS), action="append")
    args=parser.parse_args()
    if args.ndk is None:
        parser.error("Set ANDROID_NDK_HOME or provide --ndk with an installed NDK path")
    host={"Linux":"linux-x86_64", "Darwin":"darwin-x86_64"}.get(platform.system())
    if host is None:
        parser.error("Run this build script on Linux or macOS")
    metadata=json.loads(subprocess.check_output(["cargo","metadata","--locked","--format-version","1","--no-deps"],cwd=ROOT))
    target=Path(metadata["target_directory"])
    run("cargo","build","--locked","--features","cli")
    dynamic=target/"debug"/("liblibopenzilo.dylib" if platform.system()=="Darwin" else "liblibopenzilo.so")
    generated=ROOT/"android/sdk/build/generated/uniffi"
    # Generated namespaces must not survive a crate/binding rename.
    if generated.exists():
        shutil.rmtree(generated)
    generated.mkdir(parents=True,exist_ok=True)
    run(target/"debug/uniffi-bindgen","generate","--library",dynamic,"--language","kotlin","--out-dir",generated,"--config",ROOT/"uniffi.toml","--no-format")
    native_output=ROOT/"android/sdk/build/generated/jniLibs"
    if native_output.exists():
        shutil.rmtree(native_output)
    for abi in args.abi or ["arm64-v8a"]:
        rust,clang=ABIS[abi]
        linker=args.ndk.resolve()/"toolchains/llvm/prebuilt"/host/"bin"/(clang+"26-clang")
        if not linker.is_file():
            parser.error(f"NDK compiler does not exist: {linker}")
        env=os.environ.copy()
        env["CARGO_TARGET_"+rust.upper().replace("-","_")+"_LINKER"]=str(linker)
        run("cargo","build","--locked","--release","--lib","--target",rust,env=env)
        destination=native_output/abi
        destination.mkdir(parents=True,exist_ok=True)
        shutil.copy2(target/rust/"release/liblibopenzilo.so",destination)
    print("Native libraries and Kotlin bindings ready")

if __name__=="__main__": main()
