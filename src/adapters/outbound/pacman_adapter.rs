use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use crate::adapters::outbound::system_process::{CommandRunnerPort, PrivilegeDetectorPort};
use crate::domain::error::DomainError;
use crate::domain::models::{InstalledPackageSummary, PackageName, PkgInfoMetadata};
use crate::domain::ports::{ArchPackageRepository, SystemPackageManagerPort};

pub struct PacmanAdapter<'a> {
    runner: &'a dyn CommandRunnerPort,
    privileges: &'a dyn PrivilegeDetectorPort,
    installed_exact: Mutex<Option<HashSet<String>>>,
    sync_exact: Mutex<Option<HashSet<String>>>,
    probe_cache: Mutex<HashMap<String, bool>>,
}

impl<'a> PacmanAdapter<'a> {
    pub fn new(
        runner: &'a dyn CommandRunnerPort,
        privileges: &'a dyn PrivilegeDetectorPort,
    ) -> Self {
        Self {
            runner,
            privileges,
            installed_exact: Mutex::new(None),
            sync_exact: Mutex::new(None),
            probe_cache: Mutex::new(HashMap::new()),
        }
    }

    fn invalidate_caches(&self) -> Result<(), DomainError> {
        let mut installed = self.installed_exact.lock().map_err(|_| DomainError::Io {
            context: "pacman installed cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;
        *installed = None;

        let mut sync = self.sync_exact.lock().map_err(|_| DomainError::Io {
            context: "pacman sync cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;
        *sync = None;

        let mut probe = self.probe_cache.lock().map_err(|_| DomainError::Io {
            context: "pacman probe cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;
        probe.clear();

        Ok(())
    }

    fn ensure_installed_set_contains(&self, pkg: &str) -> Result<bool, DomainError> {
        let mut guard = self.installed_exact.lock().map_err(|_| DomainError::Io {
            context: "pacman installed cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;

        if guard.is_none() {
            let output = self
                .runner
                .run_capture("pacman", &["-Qq"], &[("LC_ALL", "C")])?;
            let set = if output.success {
                output
                    .stdout
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect()
            } else {
                HashSet::new()
            };
            *guard = Some(set);
        }

        Ok(guard.as_ref().is_some_and(|set| set.contains(pkg)))
    }

    fn ensure_sync_set_contains(&self, pkg: &str) -> Result<bool, DomainError> {
        let mut guard = self.sync_exact.lock().map_err(|_| DomainError::Io {
            context: "pacman sync cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;

        if guard.is_none() {
            let output = self
                .runner
                .run_capture("pacman", &["-Slq"], &[("LC_ALL", "C")])?;
            let set = if output.success {
                output
                    .stdout
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect()
            } else {
                HashSet::new()
            };
            *guard = Some(set);
        }

        Ok(guard.as_ref().is_some_and(|set| set.contains(pkg)))
    }

    fn lookup_cached(&self, pkg: &str) -> Result<Option<bool>, DomainError> {
        let guard = self.probe_cache.lock().map_err(|_| DomainError::Io {
            context: "pacman probe cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;
        Ok(guard.get(pkg).copied())
    }

    fn store_cached(&self, pkg: &str, exists: bool) -> Result<(), DomainError> {
        let mut guard = self.probe_cache.lock().map_err(|_| DomainError::Io {
            context: "pacman probe cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;
        guard.insert(pkg.to_string(), exists);
        Ok(())
    }

    fn probe_pacman_package(&self, pkg: &str) -> Result<bool, DomainError> {
        if pkg.starts_with('-') {
            return Ok(false);
        }

        if self.ensure_installed_set_contains(pkg)? || self.ensure_sync_set_contains(pkg)? {
            return Ok(true);
        }

        let local_virtual =
            self.runner
                .run_capture("pacman", &["-Qq", "--", pkg], &[("LC_ALL", "C")])?;
        if local_virtual.success {
            return Ok(true);
        }

        let sync_info =
            self.runner
                .run_capture("pacman", &["-Si", "--", pkg], &[("LC_ALL", "C")])?;
        if sync_info.success {
            return Ok(true);
        }

        let sync_virtual = self.runner.run_capture(
            "pacman",
            &["-Sddp", "--print-format", "%n", "--", pkg],
            &[("LC_ALL", "C")],
        )?;
        Ok(sync_virtual.success)
    }

    fn build_install_args<'b>(
        &self,
        package_archive: &'b str,
        auto_confirm: bool,
    ) -> (&'static str, Vec<&'b str>) {
        let mut args = Vec::new();
        if self.privileges.is_root() {
            args.push("-U");
            if auto_confirm {
                args.push("--noconfirm");
            }
            args.push("--");
            args.push(package_archive);
            ("pacman", args)
        } else {
            args.push("pacman");
            args.push("-U");
            if auto_confirm {
                args.push("--noconfirm");
            }
            args.push("--");
            args.push(package_archive);
            ("sudo", args)
        }
    }

    fn build_uninstall_args<'b>(
        &self,
        pkg_name: &'b str,
        keep_deps: bool,
        auto_confirm: bool,
    ) -> (&'static str, Vec<&'b str>) {
        let mode_flag = if keep_deps { "-R" } else { "-Rs" };
        let mut args = Vec::new();
        if self.privileges.is_root() {
            args.push(mode_flag);
            if auto_confirm {
                args.push("--noconfirm");
            }
            args.push("--");
            args.push(pkg_name);
            ("pacman", args)
        } else {
            args.push("pacman");
            args.push(mode_flag);
            if auto_confirm {
                args.push("--noconfirm");
            }
            args.push("--");
            args.push(pkg_name);
            ("sudo", args)
        }
    }
}

impl ArchPackageRepository for PacmanAdapter<'_> {
    fn package_exists(&self, package_name: &str) -> Result<bool, DomainError> {
        if let Some(cached) = self.lookup_cached(package_name)? {
            return Ok(cached);
        }
        let exists = self.probe_pacman_package(package_name)?;
        self.store_cached(package_name, exists)?;
        Ok(exists)
    }
}

impl SystemPackageManagerPort for PacmanAdapter<'_> {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError> {
        let output = self.runner.run_capture(
            "pacman",
            &["-Q", "--", package_name.as_str()],
            &[("LC_ALL", "C")],
        )?;
        Ok(output.success)
    }

    fn install_local_package(
        &self,
        package_archive: &Path,
        auto_confirm: bool,
    ) -> Result<(), DomainError> {
        let archive_str = package_archive.to_string_lossy();
        let (program, args) = self.build_install_args(&archive_str, auto_confirm);
        self.runner.run_interactive(program, &args, &[])?;
        self.invalidate_caches()?;
        Ok(())
    }

    fn uninstall_package(
        &self,
        package_name: &PackageName,
        keep_deps: bool,
        auto_confirm: bool,
    ) -> Result<(), DomainError> {
        let (program, args) =
            self.build_uninstall_args(package_name.as_str(), keep_deps, auto_confirm);
        self.runner.run_interactive(program, &args, &[])?;
        self.invalidate_caches()?;
        Ok(())
    }

    fn list_managed_packages(
        &self,
        packager_marker: &str,
    ) -> Result<Vec<InstalledPackageSummary>, DomainError> {
        let output = self
            .runner
            .run_capture("pacman", &["-Qi"], &[("LC_ALL", "C")])?;
        if !output.success {
            return Err(DomainError::CommandExecutionFailed {
                command: "pacman -Qi".to_string(),
                reason: output.stderr.trim().to_string(),
            });
        }
        Ok(parse_pacman_qi_output(&output.stdout, packager_marker))
    }

    fn install_command_preview(&self, package_archive: &Path, auto_confirm: bool) -> String {
        let archive_str = package_archive.to_string_lossy();
        let (program, args) = self.build_install_args(&archive_str, auto_confirm);
        format!("{program} {}", args.join(" "))
    }

    fn uninstall_command_preview(
        &self,
        package_name: &PackageName,
        keep_deps: bool,
        auto_confirm: bool,
    ) -> String {
        let (program, args) =
            self.build_uninstall_args(package_name.as_str(), keep_deps, auto_confirm);
        format!("{program} {}", args.join(" "))
    }
}

pub fn parse_pacman_qi_output(stdout: &str, packager_marker: &str) -> Vec<InstalledPackageSummary> {
    let mut results = Vec::new();
    let mut current: HashMap<String, String> = HashMap::new();
    let mut last_key: Option<String> = None;

    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            flush_qi_stanza(&mut current, packager_marker, &mut results);
            last_key = None;
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(key) = last_key.as_ref()
                && let Some(val) = current.get_mut(key)
            {
                val.push(' ');
                val.push_str(line.trim());
            }
            continue;
        }

        if let Some((raw_k, raw_v)) = line.split_once(':') {
            let key = raw_k.trim().to_ascii_lowercase();
            let val = raw_v.trim().to_string();
            current.insert(key.clone(), val);
            last_key = Some(key);
        }
    }

    flush_qi_stanza(&mut current, packager_marker, &mut results);
    results
}

fn flush_qi_stanza(
    stanza: &mut HashMap<String, String>,
    packager_marker: &str,
    results: &mut Vec<InstalledPackageSummary>,
) {
    if stanza.is_empty() {
        return;
    }

    let packager = stanza.get("packager").cloned().unwrap_or_default();
    let matches_packager =
        packager == packager_marker || packager.contains(PkgInfoMetadata::PACKAGER_EMAIL_MARKER);
    if matches_packager
        && let (Some(name), Some(version)) = (stanza.get("name"), stanza.get("version"))
    {
        results.push(InstalledPackageSummary {
            name: name.clone(),
            version: version.clone(),
            architecture: stanza.get("architecture").cloned().unwrap_or_default(),
            description: stanza.get("description").cloned().unwrap_or_default(),
            packager,
        });
    }

    stanza.clear();
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::Path;

    use crate::adapters::outbound::system_process::{
        CommandOutput, CommandRunnerPort, PrivilegeDetectorPort,
    };
    use crate::domain::error::DomainError;
    use crate::domain::models::{PackageName, PkgInfoMetadata};
    use crate::domain::ports::{ArchPackageRepository, SystemPackageManagerPort};

    use super::{PacmanAdapter, parse_pacman_qi_output};

    struct StubPrivileges {
        root: bool,
    }

    impl PrivilegeDetectorPort for StubPrivileges {
        fn is_root(&self) -> bool {
            self.root
        }
    }

    struct MockRunner {
        capture_responses: HashMap<String, Result<CommandOutput, DomainError>>,
        captured_calls: RefCell<Vec<String>>,
        interactive_calls: RefCell<Vec<String>>,
        fail_interactive: bool,
    }

    impl MockRunner {
        fn new() -> Self {
            Self {
                capture_responses: HashMap::new(),
                captured_calls: RefCell::new(Vec::new()),
                interactive_calls: RefCell::new(Vec::new()),
                fail_interactive: false,
            }
        }

        fn stub_capture(&mut self, key: &str, success: bool, stdout: &str) {
            self.capture_responses.insert(
                key.to_string(),
                Ok(CommandOutput {
                    status_code: Some(if success { 0 } else { 1 }),
                    success,
                    stdout: stdout.to_string(),
                    stderr: String::new(),
                }),
            );
        }
    }

    impl CommandRunnerPort for MockRunner {
        fn run_capture(
            &self,
            program: &str,
            args: &[&str],
            _env: &[(&str, &str)],
        ) -> Result<CommandOutput, DomainError> {
            let key = format!("{program} {}", args.join(" "));
            self.captured_calls.borrow_mut().push(key.clone());
            match self.capture_responses.get(&key) {
                Some(res) => res.clone(),
                None => Ok(CommandOutput {
                    status_code: Some(1),
                    success: false,
                    stdout: String::new(),
                    stderr: "not found".to_string(),
                }),
            }
        }

        fn run_interactive(
            &self,
            program: &str,
            args: &[&str],
            _env: &[(&str, &str)],
        ) -> Result<i32, DomainError> {
            let key = format!("{program} {}", args.join(" "));
            self.interactive_calls.borrow_mut().push(key.clone());
            if self.fail_interactive {
                return Err(DomainError::CommandExecutionFailed {
                    command: key,
                    reason: "mock failure".to_string(),
                });
            }
            Ok(0)
        }
    }

    #[test]
    fn caches_package_existence_resolves_virtuals_and_invalidates_after_mutation()
    -> Result<(), DomainError> {
        let mut runner = MockRunner::new();
        runner.stub_capture("pacman -Qq", true, "glibc\ngtk3\n");
        runner.stub_capture(
            "pacman -Slq",
            true,
            "webkit2gtk-4.1\nlibayatana-appindicator\n",
        );
        runner.stub_capture(
            "pacman -Sddp --print-format %n -- sdl2",
            true,
            "sdl2-compat\n",
        );

        let privs = StubPrivileges { root: false };
        let adapter = PacmanAdapter::new(&runner, &privs);

        assert_eq!(adapter.package_exists("gtk3"), Ok(true));
        assert_eq!(adapter.package_exists("webkit2gtk-4.1"), Ok(true));
        assert_eq!(adapter.package_exists("sdl2"), Ok(true));
        assert_eq!(adapter.package_exists("nonexistent"), Ok(false));
        assert_eq!(adapter.package_exists("-evil"), Ok(false));

        let call_count_before = runner.captured_calls.borrow().len();
        assert_eq!(adapter.package_exists("nonexistent"), Ok(false));
        assert_eq!(adapter.package_exists("sdl2"), Ok(true));
        assert_eq!(runner.captured_calls.borrow().len(), call_count_before);

        let pkg = PackageName::parse_and_sanitize("demo-app")?;
        adapter.install_local_package(Path::new("/tmp/demo.pkg.tar.zst"), true)?;
        let calls_after_install = runner.captured_calls.borrow().len();
        assert_eq!(adapter.package_exists("gtk3"), Ok(true));
        assert!(runner.captured_calls.borrow().len() > calls_after_install);

        adapter.uninstall_package(&pkg, false, true)?;
        let calls_after_uninstall = runner.captured_calls.borrow().len();
        assert_eq!(adapter.package_exists("gtk3"), Ok(true));
        assert!(runner.captured_calls.borrow().len() > calls_after_uninstall);
        Ok(())
    }

    #[test]
    fn uses_double_dash_and_sudo_only_when_not_root_for_install_and_uninstall()
    -> Result<(), DomainError> {
        let runner = MockRunner::new();
        let non_root = StubPrivileges { root: false };
        let adapter = PacmanAdapter::new(&runner, &non_root);

        let pkg = PackageName::parse_and_sanitize("demo-app")?;
        let _ = adapter.is_installed(&pkg)?;
        adapter.install_local_package(Path::new("/tmp/demo.pkg.tar.zst"), true)?;
        adapter.uninstall_package(&pkg, false, true)?;
        adapter.uninstall_package(&pkg, true, false)?;

        assert_eq!(
            runner.captured_calls.borrow().as_slice(),
            &["pacman -Q -- demo-app".to_string()]
        );

        let calls = runner.interactive_calls.borrow();
        assert_eq!(
            calls.as_slice(),
            &[
                "sudo pacman -U --noconfirm -- /tmp/demo.pkg.tar.zst".to_string(),
                "sudo pacman -Rs --noconfirm -- demo-app".to_string(),
                "sudo pacman -R -- demo-app".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn executes_pacman_directly_when_already_root() -> Result<(), DomainError> {
        let runner = MockRunner::new();
        let root = StubPrivileges { root: true };
        let adapter = PacmanAdapter::new(&runner, &root);

        let pkg = PackageName::parse_and_sanitize("demo-app")?;
        adapter.install_local_package(Path::new("/tmp/demo.pkg.tar.zst"), false)?;
        adapter.uninstall_package(&pkg, false, false)?;

        let calls = runner.interactive_calls.borrow();
        assert_eq!(
            calls.as_slice(),
            &[
                "pacman -U -- /tmp/demo.pkg.tar.zst".to_string(),
                "pacman -Rs -- demo-app".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn parses_pacman_qi_requiring_exact_packager_or_marker_email() {
        let stdout = "\
Name            : regular-pkg
Version         : 1.0-1
Architecture    : x86_64
Description     : Standard repo package
Packager        : Arch Linux <arch@archlinux.org>

Name            : false-positive-pkg
Version         : 1.0-1
Architecture    : x86_64
Description     : Mentions tool name in packager note
Packager        : Someone Else (tested install-in-arch) <someone@example.org>

Name            : epic-games-launcher
Version         : 1.2.3-1
Architecture    : x86_64
Description     : Epic launcher
                  wrapped from deb
Packager        : install-in-arch <install-in-arch@local>
";

        let found = parse_pacman_qi_output(stdout, PkgInfoMetadata::DEFAULT_PACKAGER);
        assert_eq!(found.len(), 1);
        if let Some(pkg) = found.first() {
            assert_eq!(pkg.name, "epic-games-launcher");
            assert_eq!(pkg.version, "1.2.3-1");
            assert_eq!(pkg.architecture, "x86_64");
            assert_eq!(pkg.description, "Epic launcher wrapped from deb");
        }
    }
}
