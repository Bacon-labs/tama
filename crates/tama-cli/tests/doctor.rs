use std::process::Command;

#[cfg(unix)]
fn write_executable(path: &std::path::Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, text).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
#[test]
fn doctor_with_root_uses_project_lean_toolchain_for_version_probes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        root.join("tama.toml"),
        "[project]\nname = \"x\"\nverity = \"v\"\n\n[yul]\nsolc = \"0.8.33\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("lakefile.toml"),
        "name = \"x\"\nbuildDir = \"artifacts/lean\"\n",
    )
    .unwrap();
    std::fs::write(root.join("lean-toolchain"), "leanprover/lean4:v4.22.0\n").unwrap();
    write_executable(
        &bin.join("lean"),
        "#!/bin/sh\n\
         if [ -f lean-toolchain ]; then\n\
         echo 'Lean (version 4.22.0, test)'\n\
         else\n\
         echo 'Lean (version 4.29.1, test)'\n\
         fi\n",
    );
    write_executable(
        &bin.join("lake"),
        "#!/bin/sh\n\
         if [ -f lean-toolchain ]; then\n\
         echo 'Lake version 5.0.0-src+test (Lean version 4.22.0)'\n\
         else\n\
         echo 'Lake version 5.0.0-src+test (Lean version 4.29.1)'\n\
         fi\n",
    );
    write_executable(
        &bin.join("solc"),
        "#!/bin/sh\necho 'Version: 0.8.33+test'\n",
    );
    write_executable(
        &bin.join("forge"),
        "#!/bin/sh\necho 'forge Version: 1.6.0-test'\n",
    );
    write_executable(&bin.join("git"), "#!/bin/sh\necho 'git version test'\n");
    write_executable(&bin.join("tar"), "#!/bin/sh\necho 'bsdtar test'\n");

    let output = Command::new(env!("CARGO_BIN_EXE_tama"))
        .arg("--color=never")
        .arg("--root")
        .arg(&root)
        .arg("doctor")
        .env("PATH", &bin)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("ok   lean            4.22.0"));
    assert!(stdout.contains("ok   lake            5.0.0-src+test"));
    assert!(stdout.contains("Lean version 4.22.0"));
    assert!(!stdout.contains("fail lean"));
    assert!(!stdout.contains("fail lake"));
    assert!(!stdout.contains("4.29.1"));
}
