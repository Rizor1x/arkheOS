use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;
const TARGET: &str = "x86_64-unknown-none";
const MEMORY_CHECK_MARKER: &str = "memory: executable sealing rollback and reuse verified";
const OWNERSHIP_CHECK_MARKER: &str = "memory: kernel and user root ownership verified";

fn main() {
    if let Err(error) = dispatch() {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn dispatch() -> Result {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("missing workspace")?;
    env::set_current_dir(root)?;
    let args: Vec<String> = env::args().skip(1).collect();
    let task = args.first().map(String::as_str).unwrap_or("help");
    match task {
        "help" if args.len() <= 1 => {
            println!(
                "cargo xtask <check|build|fetch-bootloader|iso|run|smoke|smoke-exceptions|smoke-mappings|smoke-userspace|smoke-scheduler|smoke-ipc|smoke-processes|audit|hooks|doctor>"
            );
            Ok(())
        }
        _ if args.len() != 1 => Err("expected exactly one task; use cargo xtask help".into()),
        "check" => check(),
        "build" => build(false, "boot").map(|_| ()),
        "fetch-bootloader" => fetch_bootloader(),
        "iso" => iso(false, "boot").map(|_| ()),
        "run" => run_qemu(),
        "smoke" => smoke("boot", "ARKHE_USER_OK"),
        "smoke-mappings" => {
            for case in [
                "kernel-write-code",
                "kernel-write-alias",
                "kernel-exec-data",
                "kernel-stack-guard",
                "kernel-write-module",
                "user-code-alias",
                "user-code-alias-switched",
            ] {
                smoke(case, "ARKHE_KERNEL_MAPPING_OK")?;
            }
            Ok(())
        }
        "smoke-processes" => smoke("spawn", "ARKHE_PROCESS_OK"),
        "smoke-ipc" => {
            smoke("ipc", "ARKHE_IPC_OK")?;
            smoke("ipc-peer-fault", "ARKHE_IPC_PEER_FAULT_OK")
        }
        "smoke-scheduler" => {
            smoke("scheduler", "ARKHE_SCHEDULER_OK")?;
            smoke("idle", "ARKHE_IDLE_OK")
        }
        "smoke-userspace" => {
            for (case, marker) in [
                ("user-kernel-read", "ARKHE_USER_KERNEL_READ_OK"),
                ("user-cli", "ARKHE_USER_CLI_OK"),
                ("user-write-code", "ARKHE_USER_WRITE_CODE_OK"),
                ("user-exec-stack", "ARKHE_USER_EXEC_STACK_OK"),
                ("user-fpu", "ARKHE_USER_FPU_OK"),
            ] {
                smoke(case, marker)?;
            }
            Ok(())
        }
        "smoke-exceptions" => {
            for (case, marker) in [
                ("divide", "ARKHE_DIVIDE_OK"),
                ("page-fault", "ARKHE_PAGE_FAULT_OK"),
                ("double-fault", "ARKHE_DOUBLE_FAULT_OK"),
            ] {
                smoke(case, marker)?;
            }
            Ok(())
        }
        "audit" => run(Command::new("python3").arg("tools/audit_repo.py")),
        "hooks" => {
            ensure_git_root()?;
            run(Command::new("git").args(["config", "--local", "core.hooksPath", ".githooks"]))
        }
        "doctor" => doctor(),
        _ => Err("unknown task; use cargo xtask help".into()),
    }
}

fn run(command: &mut Command) -> Result {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if !status.success() {
        return Err(format!("{program} failed: {status}").into());
    }
    Ok(())
}

fn check() -> Result {
    run(Command::new("cargo").args(["fmt", "--all", "--", "--check"]))?;
    run(Command::new("rustfmt").args(["--edition", "2024", "--check", "tests/paging_api.rs"]))?;
    run(Command::new("cargo").args(["test", "--locked"]))?;
    run(Command::new("cargo").args([
        "clippy",
        "--locked",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(Command::new("cargo").args([
        "clippy",
        "--locked",
        "--package",
        "arkhe-kernel",
        "--target",
        TARGET,
        "--all-features",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(Command::new("python3").args(["-m", "unittest", "discover", "-s", "tests"]))?;
    run(Command::new("cargo")
        .env(
            "RUSTFLAGS",
            "-C relocation-model=static -C code-model=small",
        )
        .args([
            "clippy",
            "--locked",
            "--package",
            "arkhe-init",
            "--target",
            TARGET,
            "--",
            "-D",
            "warnings",
        ]))?;
    build_userspace("boot")?;
    build(false, "boot")?;
    run(Command::new("python3").arg("tools/check_paging_api.py"))?;
    run(Command::new("python3").arg("tools/audit_repo.py"))
}

fn build(smoke: bool, case: &str) -> Result<PathBuf> {
    let directory = if smoke {
        format!("target/smoke-{case}")
    } else {
        "target/kernel".into()
    };
    let mut command = Command::new("cargo");
    command.args([
        "build",
        "--locked",
        "--release",
        "--package",
        "arkhe-kernel",
        "--target",
        TARGET,
        "--target-dir",
        &directory,
    ]);
    command.env("ARKHE_TEST_CASE", case);
    if smoke {
        command.args(["--features", "smoke"]);
    }
    run(&mut command)?;
    let kernel = Path::new(&directory)
        .join(TARGET)
        .join("release/arkhe-kernel");
    run(Command::new("python3")
        .arg("tools/check_elf.py")
        .arg(&kernel))?;
    Ok(kernel)
}

fn build_userspace(case: &str) -> Result<PathBuf> {
    let directory = format!("target/user-{case}");
    run(Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--release",
            "--package",
            "arkhe-init",
            "--target",
            TARGET,
            "--target-dir",
            &directory,
        ])
        .env("ARKHE_TEST_CASE", case)
        .env("CARGO_PROFILE_RELEASE_LTO", "false")
        .env(
            "RUSTFLAGS",
            "-C relocation-model=static -C code-model=small",
        ))?;
    let binaries = Path::new(&directory).join(TARGET).join("release");
    for name in ["arkhe-init", "arkhe-supervisor", "arkhe-worker"] {
        run(Command::new("python3")
            .arg("tools/check_elf.py")
            .arg(binaries.join(name)))?;
    }
    Ok(binaries)
}

fn lock_value(key: &str) -> Result<String> {
    let lock = fs::read_to_string("boot/limine.lock")?;
    for line in lock.lines() {
        if let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Ok(value.trim().trim_matches('"').to_owned());
        }
    }
    Err(format!("missing {key} in boot/limine.lock").into())
}

fn fetch_bootloader() -> Result {
    let path = Path::new(".cache/limine");
    let revision = lock_value("revision")?;
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid Limine revision".into());
    }
    if path.exists() {
        verify_bootloader()?;
        println!("Limine cache matches the pinned revision");
        return Ok(());
    }
    fs::create_dir_all(path)?;
    run(Command::new("git").arg("init").arg(path))?;
    let repository = lock_value("repository")?;
    run(Command::new("git").arg("-C").arg(path).args([
        "fetch",
        "--depth=1",
        &repository,
        &revision,
    ]))?;
    run(Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["checkout", "--detach", "FETCH_HEAD"]))?;
    verify_bootloader()
}

fn verify_bootloader() -> Result {
    let path = Path::new(".cache/limine");
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !output.status.success()
        || String::from_utf8(output.stdout)?.trim() != lock_value("revision")?
    {
        return Err(
            "Limine cache does not match boot/limine.lock; move it aside and fetch again".into(),
        );
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()?;
    if !output.status.success() || !output.stdout.is_empty() {
        return Err("Limine tracked files have local changes".into());
    }
    for file in [
        "limine.c",
        "limine-bios.sys",
        "limine-bios-cd.bin",
        "limine-uefi-cd.bin",
        "BOOTX64.EFI",
    ] {
        if !path.join(file).is_file() {
            return Err(format!("Limine is incomplete: {file}").into());
        }
    }
    Ok(())
}

fn iso(smoke: bool, case: &str) -> Result<PathBuf> {
    verify_bootloader()?;
    let kernel = build(smoke, case)?;
    let directory = PathBuf::from(if smoke {
        format!("build/smoke-{case}")
    } else {
        "build/normal".into()
    });
    let staging = directory.join("iso_root");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let boot = staging.join("boot/limine");
    let efi = staging.join("EFI/BOOT");
    fs::create_dir_all(&boot)?;
    fs::create_dir_all(&efi)?;
    fs::copy(kernel, staging.join("boot/arkhe-kernel"))?;
    let binaries = build_userspace(case)?;
    let mut archive = Command::new("python3");
    archive
        .arg("tools/mkinitramfs.py")
        .arg(staging.join("boot/initramfs.tar"))
        .arg(binaries.join("arkhe-init"))
        .arg(binaries.join("arkhe-supervisor"))
        .arg(binaries.join("arkhe-worker"));
    if smoke && case == "spawn" {
        archive.arg("--invalid-executable");
    }
    run(&mut archive)?;
    fs::copy("boot/limine.conf", boot.join("limine.conf"))?;
    for file in [
        "limine-bios.sys",
        "limine-bios-cd.bin",
        "limine-uefi-cd.bin",
    ] {
        fs::copy(Path::new(".cache/limine").join(file), boot.join(file))?;
    }
    fs::copy(".cache/limine/BOOTX64.EFI", efi.join("BOOTX64.EFI"))?;
    let installer = directory.join("limine-host");
    run(Command::new("cc")
        .args(["-O2", "-std=c99", ".cache/limine/limine.c", "-o"])
        .arg(&installer))?;
    let image = directory.join("arkheOS.iso");
    run(Command::new("xorriso")
        .args([
            "-as",
            "mkisofs",
            "-R",
            "-r",
            "-J",
            "-b",
            "boot/limine/limine-bios-cd.bin",
            "-no-emul-boot",
            "-boot-load-size",
            "4",
            "-boot-info-table",
            "-hfsplus",
            "-apm-block-size",
            "2048",
            "--efi-boot",
            "boot/limine/limine-uefi-cd.bin",
            "-efi-boot-part",
            "--efi-boot-image",
            "--protective-msdos-label",
        ])
        .arg(&staging)
        .arg("-o")
        .arg(&image))?;
    run(Command::new(installer).arg("bios-install").arg(&image))?;
    println!("image: {}", image.display());
    Ok(image)
}

fn qemu(image: &Path) -> Result<Command> {
    let mut command = Command::new("qemu-system-x86_64");
    command
        .args([
            "-accel",
            "tcg",
            "-machine",
            "q35",
            "-cpu",
            "qemu64",
            "-smp",
            "1",
            "-m",
            "256M",
            "-display",
            "none",
            "-monitor",
            "none",
            "-net",
            "none",
            "-no-reboot",
            "-boot",
            "d",
        ])
        .arg("-cdrom")
        .arg(image);
    match (env::var_os("ARKHE_OVMF"), env::var_os("ARKHE_OVMF_VARS")) {
        (Some(code), Some(template)) => {
            if !Path::new(&code).is_file() || !Path::new(&template).is_file() {
                return Err("UEFI code or variable template does not exist".into());
            }
            let variables = image
                .parent()
                .ok_or("image has no directory")?
                .join("uefi-vars.fd");
            fs::copy(&template, &variables)?;
            let code = Path::new(&code).display().to_string().replace(',', ",,");
            let variables = variables.display().to_string().replace(',', ",,");
            command.arg("-drive").arg(format!(
                "if=pflash,unit=0,format=raw,readonly=on,file={code}"
            ));
            command
                .arg("-drive")
                .arg(format!("if=pflash,unit=1,format=raw,file={variables}"));
        }
        (None, None) => {}
        _ => return Err("UEFI requires both ARKHE_OVMF and ARKHE_OVMF_VARS".into()),
    }
    Ok(command)
}

fn run_qemu() -> Result {
    let image = iso(false, "boot")?;
    println!("Serial console; stop QEMU with Ctrl+C");
    run(qemu(&image)?.args(["-serial", "stdio"]))
}

fn smoke(case: &str, marker: &str) -> Result {
    let image = iso(true, case)?;
    let log_path = image
        .parent()
        .ok_or("image has no directory")?
        .join("serial.log");
    let log = fs::File::create(&log_path)?;
    let mut child = qemu(&image)?
        .args([
            "-device",
            "isa-debug-exit,iobase=0xf4,iosize=0x04",
            "-serial",
            "stdio",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::inherit())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(
                format!("QEMU {case} test timed out; inspect {}", log_path.display()).into(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    };
    let text = fs::read_to_string(&log_path)?;
    if status.code() != Some(33) || !smoke_output_valid(case, marker, &text) {
        return Err(format!(
            "{case} check failed ({status}); inspect {}",
            log_path.display()
        )
        .into());
    }
    println!("QEMU {case} passed: {marker}");
    Ok(())
}

fn smoke_output_valid(case: &str, marker: &str, text: &str) -> bool {
    let orphan_verified = !matches!(case, "boot" | "spawn")
        || text
            .lines()
            .filter(|line| line.trim() == "service: orphan observed parent closure")
            .count()
            == 2;
    orphan_verified
        && text.lines().any(|line| line.trim() == MEMORY_CHECK_MARKER)
        && text
            .lines()
            .any(|line| line.trim() == OWNERSHIP_CHECK_MARKER)
        && text.lines().any(|line| line.trim() == marker)
        && !text.contains("PANIC:")
}

fn ensure_git_root() -> Result {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !output.status.success()
        || Path::new(String::from_utf8(output.stdout)?.trim()).canonicalize()?
            != env::current_dir()?.canonicalize()?
    {
        return Err("workspace must have its own Git repository".into());
    }
    Ok(())
}

fn doctor() -> Result {
    for (program, flag) in [
        ("rustc", "--version"),
        ("cargo", "--version"),
        ("python3", "--version"),
        ("git", "--version"),
        ("cc", "--version"),
        ("xorriso", "-version"),
        ("qemu-system-x86_64", "--version"),
    ] {
        run(Command::new(program).arg(flag))?;
    }
    run(Command::new("rustup").args(["target", "list", "--installed"]))?;
    ensure_git_root()
}

#[cfg(test)]
mod tests {
    use super::{MEMORY_CHECK_MARKER, OWNERSHIP_CHECK_MARKER, smoke_output_valid};

    #[test]
    fn process_smoke_requires_both_unwatched_child_confirmations() {
        let marker = "ARKHE_PROCESS_OK";
        let one = "service: orphan observed parent closure\r\n";
        let memory = format!("{MEMORY_CHECK_MARKER}\r\n{OWNERSHIP_CHECK_MARKER}\r\n");
        assert!(!smoke_output_valid(
            "spawn",
            marker,
            &format!("{memory}{marker}")
        ));
        assert!(!smoke_output_valid(
            "spawn",
            marker,
            &format!("{memory}{one}{marker}")
        ));
        let complete = format!("{memory}{one}{one}{marker}");
        assert!(smoke_output_valid("spawn", marker, &complete));
        assert!(smoke_output_valid("boot", marker, &complete));
        assert!(!smoke_output_valid(
            "spawn",
            marker,
            &format!("{complete}\nPANIC:")
        ));
        assert!(!smoke_output_valid(
            "spawn",
            marker,
            &format!("{memory}{one}{one}")
        ));
        assert!(smoke_output_valid(
            "ipc",
            marker,
            &format!("{memory}{marker}")
        ));
    }

    #[test]
    fn smoke_requires_completed_memory_and_ownership_checks() {
        let marker = "ARKHE_KERNEL_MAPPING_OK";
        for case in ["kernel-write-code", "user-code-alias", "ipc", "divide"] {
            assert!(!smoke_output_valid(case, marker, marker));
            assert!(!smoke_output_valid(
                case,
                marker,
                &format!("{MEMORY_CHECK_MARKER}\n{marker}")
            ));
            assert!(!smoke_output_valid(
                case,
                marker,
                &format!("{OWNERSHIP_CHECK_MARKER}\n{marker}")
            ));
            assert!(!smoke_output_valid(
                case,
                marker,
                &format!("prefix {MEMORY_CHECK_MARKER}\n{OWNERSHIP_CHECK_MARKER}\n{marker}")
            ));
            assert!(!smoke_output_valid(case, marker, MEMORY_CHECK_MARKER));
            assert!(smoke_output_valid(
                case,
                marker,
                &format!("{MEMORY_CHECK_MARKER}\r\n{OWNERSHIP_CHECK_MARKER}\r\n{marker}\r\n")
            ));
        }
    }
}
