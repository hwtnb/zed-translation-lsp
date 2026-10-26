use super::*;

fn launch(program: &str, arguments: &[&str]) -> Launch {
    Launch::new(program, arguments, &[])
}

#[test]
fn each_os_is_given_the_command_the_way_its_terminal_takes_it() {
    let script = Path::new("/tmp/synthetic key.command");
    assert_eq!(
        macos(script),
        [launch(
            "/usr/bin/open",
            &["-a", "Terminal", "/tmp/synthetic key.command"]
        )]
    );

    let executable = Path::new("/opt/synthetic server/translation-lsp");
    let command = ["/opt/synthetic server/translation-lsp", "anthropic-key"];
    let expected: Vec<_> = [
        ("xdg-terminal-exec", &[][..]),
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("ptyxis", &["--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-x"]),
        ("kitty", &[]),
        ("alacritty", &["-e"]),
        ("foot", &[]),
        ("wezterm", &["start", "--"]),
        ("xterm", &["-e"]),
    ]
    .into_iter()
    .map(|(program, options)| launch(program, &[options, &command].concat()))
    .collect();
    assert_eq!(linux(executable), expected);

    let executable = Path::new(r"C:\Synthetic Server\translation-lsp.exe");
    assert_eq!(
        windows(executable),
        [launch(
            r"C:\Synthetic Server\translation-lsp.exe",
            &["anthropic-key"]
        )]
    );
}

#[test]
fn script_removes_itself_and_runs_the_quoted_executable() {
    assert_eq!(
        script("/opt/translation-lsp"),
        "#!/bin/sh\nrm -f \"$0\"\nexec '/opt/translation-lsp' anthropic-key\n"
    );
    // Nothing in a path is read by the shell.
    assert_eq!(
        script("/opt/it's $HOME `id`/translation \"lsp\""),
        "#!/bin/sh\nrm -f \"$0\"\nexec '/opt/it'\\''s $HOME `id`/translation \"lsp\"' anthropic-key\n"
    );
}

#[test]
fn command_to_run_by_hand_quotes_a_path_that_needs_it() {
    let command = |path: &str| manual_command(Path::new(path));
    if cfg!(windows) {
        assert_eq!(
            command(r"C:\Users\synthetic\translation-lsp.exe"),
            r"C:\Users\synthetic\translation-lsp.exe anthropic-key"
        );
        assert_eq!(
            command(r"C:\Users\synthetic user\translation-lsp.exe"),
            r#""C:\Users\synthetic user\translation-lsp.exe" anthropic-key"#
        );
    } else {
        assert_eq!(
            command("/opt/translation-lsp_0.1+2/translation-lsp"),
            "/opt/translation-lsp_0.1+2/translation-lsp anthropic-key"
        );
        assert_eq!(
            command("/Users/synthetic/Library/Application Support/Zed/translation-lsp"),
            "'/Users/synthetic/Library/Application Support/Zed/translation-lsp' anthropic-key"
        );
        assert_eq!(
            command("/opt/it's/translation-lsp"),
            "'/opt/it'\\''s/translation-lsp' anthropic-key"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn program_that_is_not_installed_is_not_a_terminal() {
    let absent = launch("translation-lsp-synthetic-absent-terminal", &[]);
    assert!(!first_started(&[absent], Duration::ZERO).await);
    assert!(!first_started(&[], Duration::ZERO).await);
}

#[cfg(unix)]
mod started {
    use std::{env, os::unix::fs::PermissionsExt};

    use uuid::Uuid;

    use super::*;

    /// Long enough for a failing program to end on a slow machine. A program
    /// that ends is not waited for any longer than it runs.
    const PATIENT: Duration = Duration::from_secs(60);

    fn shell(script: &str) -> Launch {
        launch("/bin/sh", &["-c", script])
    }

    fn scratch() -> PathBuf {
        let directory =
            env::temp_dir().join(format!("translation-lsp-terminal-test-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        directory
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_program_that_does_not_fail_takes_the_command() {
        let directory = scratch();
        let mark = |name: &str| format!("touch '{}/{name}'", directory.display());
        let launches = [
            launch("translation-lsp-synthetic-absent-terminal", &[]),
            shell(&format!("{}; exit 3", mark("failed"))),
            shell(&mark("opened")),
            shell(&mark("later")),
        ];
        assert!(first_started(&launches, PATIENT).await);
        let mut names: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["failed", "opened"]);
        assert!(!first_started(&launches[..2], PATIENT).await);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn program_still_running_after_the_grace_has_its_window() {
        let directory = scratch();
        let (pipe, ended) = (directory.join("pipe"), directory.join("ended"));
        let made = Command::new("mkfifo").arg(&pipe).status().unwrap();
        assert!(made.success());
        // The program waits for the test to open the pipe, as a terminal waits
        // for its user. No length of time decides what the test observes.
        let waiting = shell(&format!(
            "cat '{}' && touch '{}'",
            pipe.display(),
            ended.display()
        ));
        let started = first_started(&[waiting], Duration::from_millis(100)).await;
        let running = !ended.exists();
        // Opening the pipe lets the program end, whatever was observed.
        fs::write(&pipe, "").unwrap();
        assert!(started, "A running program counts as a window that opened");
        assert!(running, "The server went on while the program was running");
        let begun = Instant::now();
        while !ended.exists() {
            assert!(begun.elapsed() < PATIENT, "The program did not end");
            thread::sleep(Duration::from_millis(5));
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn program_gets_no_stream_of_the_server_and_a_process_group_of_its_own() {
        for check in [
            "[ /dev/stdin -ef /dev/null ]",
            "[ /dev/stdout -ef /dev/null ]",
            "[ /dev/stderr -ef /dev/null ]",
            // A group with the shell's own ID exists only when the shell leads it.
            "kill -s 0 -- -$$",
        ] {
            assert!(first_started(&[shell(check)], PATIENT).await, "{check}");
        }
        // A check that does not hold is reported as a program that failed.
        assert!(!first_started(&[shell("[ /dev/stdin -ef /dev/zero ]")], PATIENT).await);
    }

    #[test]
    fn script_file_is_private_and_gone_once_it_has_run() {
        // `echo` stands for the executable: the script runs it with the command.
        let path = write_script(Path::new("/bin/echo")).unwrap();
        assert_eq!(path.parent().unwrap(), env::temp_dir());
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("translation-lsp-anthropic-key-") && name.ends_with(".command"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), script("/bin/echo"));
        let output = Command::new(&path).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"anthropic-key\n");
        assert!(!path.exists());
    }
}
