use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use crate::adapters::outbound::system_process::{CommandRunnerPort, PrivilegeDetectorPort};
use crate::domain::error::DomainError;
use crate::domain::models::{InstalledPackageSummary, PackageName};
use crate::domain::ports::{ArchPackageRepository, SystemPackageManagerPort};
use crate::domain::sanitizer::sanitize_for_error;

pub const PACMAN_BIN: &str = "/usr/bin/pacman";
pub const SUDO_BIN: &str = "/usr/bin/sudo";

pub struct PacmanAdapter<'a> {
    runner: &'a dyn CommandRunnerPort,
    privileges: &'a dyn PrivilegeDetectorPort,
    installed_exact: Mutex<Option<HashSet<String>>>,
    sync_exact: Mutex<Option<HashSet<String>>>,
    virtual_provides: Mutex<Option<HashSet<String>>>,
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
            virtual_provides: Mutex::new(None),
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

        let mut provides = self.virtual_provides.lock().map_err(|_| DomainError::Io {
            context: "pacman provides cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;
        *provides = None;

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
                .run_capture(PACMAN_BIN, &["-Qq"], &[("LC_ALL", "C")])?;
            if !output.success {
                return Err(DomainError::CommandExecutionFailed {
                    command: format!("{PACMAN_BIN} -Qq"),
                    reason: sanitize_for_error(output.stderr.trim()),
                });
            }
            let set = output
                .stdout
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
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
                .run_capture(PACMAN_BIN, &["-Slq"], &[("LC_ALL", "C")])?;
            if !output.success {
                return Err(DomainError::CommandExecutionFailed {
                    command: format!("{PACMAN_BIN} -Slq"),
                    reason: sanitize_for_error(output.stderr.trim()),
                });
            }
            let set = output
                .stdout
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            *guard = Some(set);
        }

        Ok(guard.as_ref().is_some_and(|set| set.contains(pkg)))
    }

    fn ensure_virtual_provides_contains(&self, pkg: &str) -> Result<bool, DomainError> {
        let mut guard = self.virtual_provides.lock().map_err(|_| DomainError::Io {
            context: "pacman provides cache lock".to_string(),
            reason: "mutex poisoned".to_string(),
        })?;

        if guard.is_none() {
            let mut set = HashSet::new();

            let qi_output = self
                .runner
                .run_capture(PACMAN_BIN, &["-Qi"], &[("LC_ALL", "C")])?;
            if !qi_output.success {
                return Err(DomainError::CommandExecutionFailed {
                    command: format!("{PACMAN_BIN} -Qi"),
                    reason: sanitize_for_error(qi_output.stderr.trim()),
                });
            }
            collect_names_and_provides(&qi_output.stdout, &mut set);

            let si_output = self
                .runner
                .run_capture(PACMAN_BIN, &["-Si"], &[("LC_ALL", "C")])?;
            if !si_output.success {
                return Err(DomainError::CommandExecutionFailed {
                    command: format!("{PACMAN_BIN} -Si"),
                    reason: sanitize_for_error(si_output.stderr.trim()),
                });
            }
            collect_names_and_provides(&si_output.stdout, &mut set);

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

        self.ensure_virtual_provides_contains(pkg)
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
            (PACMAN_BIN, args)
        } else {
            args.push(PACMAN_BIN);
            args.push("-U");
            if auto_confirm {
                args.push("--noconfirm");
            }
            args.push("--");
            args.push(package_archive);
            (SUDO_BIN, args)
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
            (PACMAN_BIN, args)
        } else {
            args.push(PACMAN_BIN);
            args.push(mode_flag);
            if auto_confirm {
                args.push("--noconfirm");
            }
            args.push("--");
            args.push(pkg_name);
            (SUDO_BIN, args)
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

    fn is_native_sync_package(&self, package_name: &str) -> Result<bool, DomainError> {
        if package_name.starts_with('-') {
            return Ok(false);
        }
        self.ensure_sync_set_contains(package_name)
    }
}

impl SystemPackageManagerPort for PacmanAdapter<'_> {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError> {
        Ok(self.installed_package_info(package_name)?.is_some())
    }

    fn installed_package_info(
        &self,
        package_name: &PackageName,
    ) -> Result<Option<InstalledPackageSummary>, DomainError> {
        if !self.ensure_installed_set_contains(package_name.as_str())? {
            return Ok(None);
        }
        let output = self.runner.run_capture(
            PACMAN_BIN,
            &["-Qi", "--", package_name.as_str()],
            &[("LC_ALL", "C")],
        )?;
        if !output.success {
            if output.stderr.contains("was not found") || output.stderr.trim() == "not found" {
                return Ok(None);
            }
            return Err(DomainError::CommandExecutionFailed {
                command: format!("{PACMAN_BIN} -Qi -- {}", package_name.as_str()),
                reason: sanitize_for_error(output.stderr.trim()),
            });
        }
        let stanzas = parse_all_pacman_qi_stanzas(&output.stdout);
        Ok(stanzas
            .into_iter()
            .find(|pkg| pkg.name == package_name.as_str()))
    }

    fn install_local_package(
        &self,
        package_archive: &Path,
        auto_confirm: bool,
    ) -> Result<(), DomainError> {
        let archive_str = package_archive.to_str().ok_or_else(|| DomainError::Io {
            context: "converting archive path to UTF-8".to_string(),
            reason: "archive path contains invalid UTF-8".to_string(),
        })?;
        let (program, args) = self.build_install_args(archive_str, auto_confirm);
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
            .run_capture(PACMAN_BIN, &["-Qi"], &[("LC_ALL", "C")])?;
        if !output.success {
            return Err(DomainError::CommandExecutionFailed {
                command: format!("{PACMAN_BIN} -Qi"),
                reason: sanitize_for_error(output.stderr.trim()),
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
    parse_all_pacman_qi_stanzas(stdout)
        .into_iter()
        .filter(|pkg| pkg.packager == packager_marker)
        .collect()
}

fn parse_all_pacman_qi_stanzas(stdout: &str) -> Vec<InstalledPackageSummary> {
    let mut results = Vec::new();
    let mut current: HashMap<String, String> = HashMap::new();
    let mut last_key: Option<String> = None;

    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            flush_qi_stanza(&mut current, &mut results);
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

    flush_qi_stanza(&mut current, &mut results);
    results
}

fn flush_qi_stanza(
    stanza: &mut HashMap<String, String>,
    results: &mut Vec<InstalledPackageSummary>,
) {
    if stanza.is_empty() {
        return;
    }

    let packager = stanza.get("packager").cloned().unwrap_or_default();
    if let (Some(name), Some(version)) = (stanza.get("name"), stanza.get("version")) {
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

fn collect_names_and_provides(stdout: &str, out: &mut HashSet<String>) {
    let mut in_provides = false;
    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            in_provides = false;
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if in_provides {
                insert_provide_tokens(line, out);
            }
            continue;
        }
        in_provides = false;
        if let Some((raw_k, raw_v)) = line.split_once(':') {
            let key = raw_k.trim();
            if key.eq_ignore_ascii_case("Name") {
                let name = raw_v.trim();
                if !name.is_empty() {
                    out.insert(name.to_string());
                }
            } else if key.eq_ignore_ascii_case("Provides") {
                in_provides = true;
                insert_provide_tokens(raw_v, out);
            }
        }
    }
}

fn insert_provide_tokens(raw_list: &str, out: &mut HashSet<String>) {
    for token in raw_list.split_whitespace() {
        if token == "None" {
            continue;
        }
        let base = token.split_once('=').map_or(token, |(name, _)| name);
        if !base.is_empty() {
            out.insert(base.to_string());
        }
    }
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

    use super::{PACMAN_BIN, PacmanAdapter, SUDO_BIN, parse_pacman_qi_output};

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
                    stderr: if success {
                        String::new()
                    } else {
                        "error: failed".to_string()
                    },
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
    fn caches_package_existence_batches_virtuals_and_invalidates_after_mutation()
    -> Result<(), DomainError> {
        let mut runner = MockRunner::new();
        runner.stub_capture(&format!("{PACMAN_BIN} -Qq"), true, "glibc\ngtk3\n");
        runner.stub_capture(
            &format!("{PACMAN_BIN} -Slq"),
            true,
            "webkit2gtk-4.1\nlibayatana-appindicator\n",
        );
        runner.stub_capture(
            &format!("{PACMAN_BIN} -Qi"),
            true,
            "Name : bash\nProvides : sh\n",
        );
        runner.stub_capture(
            &format!("{PACMAN_BIN} -Si"),
            true,
            "Name : sdl2-compat\nProvides : sdl2=2.32.74\n",
        );

        let privs = StubPrivileges { root: false };
        let adapter = PacmanAdapter::new(&runner, &privs);

        assert_eq!(adapter.package_exists("gtk3"), Ok(true));
        assert_eq!(adapter.package_exists("webkit2gtk-4.1"), Ok(true));
        assert_eq!(adapter.is_native_sync_package("webkit2gtk-4.1"), Ok(true));
        assert_eq!(adapter.is_native_sync_package("sdl2"), Ok(false));
        assert_eq!(adapter.package_exists("sdl2"), Ok(true));
        assert_eq!(adapter.package_exists("sh"), Ok(true));
        assert_eq!(adapter.package_exists("nonexistent"), Ok(false));
        assert_eq!(adapter.package_exists("nonexistent-2"), Ok(false));
        assert_eq!(adapter.package_exists("-evil"), Ok(false));

        let call_count_before = runner.captured_calls.borrow().len();
        assert_eq!(call_count_before, 4);
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
    fn propagates_pacman_query_errors_instead_of_silencing() {
        let mut runner = MockRunner::new();
        runner.stub_capture(&format!("{PACMAN_BIN} -Qq"), false, "");
        let privs = StubPrivileges { root: false };
        let adapter = PacmanAdapter::new(&runner, &privs);
        assert!(matches!(
            adapter.package_exists("gtk3"),
            Err(DomainError::CommandExecutionFailed { .. })
        ));
    }

    #[test]
    fn uses_absolute_paths_double_dash_and_sudo_only_when_not_root() -> Result<(), DomainError> {
        let mut runner = MockRunner::new();
        runner.stub_capture(&format!("{PACMAN_BIN} -Qq"), true, "demo-app\nbash\n");
        runner.stub_capture(
            &format!("{PACMAN_BIN} -Qi -- demo-app"),
            true,
            "Name : demo-app\nVersion : 1.0-1\nArchitecture : x86_64\nDescription : Demo\nPackager : install-in-arch <install-in-arch@local>\n",
        );
        let non_root = StubPrivileges { root: false };
        let adapter = PacmanAdapter::new(&runner, &non_root);

        let pkg = PackageName::parse_and_sanitize("demo-app")?;
        assert!(adapter.is_installed(&pkg)?);
        let virtual_pkg = PackageName::parse_and_sanitize("sh")?;
        assert!(!adapter.is_installed(&virtual_pkg)?);

        adapter.install_local_package(Path::new("/tmp/demo.pkg.tar.zst"), true)?;
        adapter.uninstall_package(&pkg, false, true)?;
        adapter.uninstall_package(&pkg, true, false)?;

        assert_eq!(
            runner.captured_calls.borrow().as_slice(),
            &[
                format!("{PACMAN_BIN} -Qq"),
                format!("{PACMAN_BIN} -Qi -- demo-app"),
            ]
        );

        let calls = runner.interactive_calls.borrow();
        assert_eq!(
            calls.as_slice(),
            &[
                format!("{SUDO_BIN} {PACMAN_BIN} -U --noconfirm -- /tmp/demo.pkg.tar.zst"),
                format!("{SUDO_BIN} {PACMAN_BIN} -Rs --noconfirm -- demo-app"),
                format!("{SUDO_BIN} {PACMAN_BIN} -R -- demo-app"),
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
                format!("{PACMAN_BIN} -U -- /tmp/demo.pkg.tar.zst"),
                format!("{PACMAN_BIN} -Rs -- demo-app"),
            ]
        );
        Ok(())
    }

    #[test]
    fn parses_pacman_qi_requiring_exact_packager_equality() {
        let stdout = "\
Name            : regular-pkg
Version         : 1.0-1
Architecture    : x86_64
Description     : Standard repo package
Packager        : Arch Linux <arch@archlinux.org>

Name            : spoofed-email-pkg
Version         : 1.0-1
Architecture    : x86_64
Description     : Spoofs marker email in packager string
Packager        : Evil Attacker <install-in-arch@local>

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
