// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Each of these is a way an install can be wrong that an init system would accept quietly.

use std::path::{Path, PathBuf};

use super::*;
use crate::artefact::{CHECKSUMS_FILE, checksums_for};
use crate::manifest::{MANIFEST_VERSION, ScheduledCheck};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-deploy-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn program(name: &str, class: ProgramClass) -> Program {
    Program {
        name: name.to_string(),
        binary: format!("/opt/bijective/bin/bijective-{name}"),
        class,
        arguments: vec!["--store".into(), "/var/lib/bijective".into()],
        after: Vec::new(),
        memory_max_mb: 512,
        stop_timeout_seconds: 30,
        delegates_cgroup: false,
        writes: Vec::new(),
        credentials: Vec::new(),
    }
}

fn deployment(programs: Vec<Program>) -> Deployment {
    Deployment {
        version: MANIFEST_VERSION,
        platform_version: "0.1.0".to_string(),
        programs,
        checks: Vec::new(),
    }
}

/// A check with no instance, since a name here would be a venue in the codebase.
fn check() -> ScheduledCheck {
    ScheduledCheck {
        name: "assurance".to_string(),
        binary: "/usr/local/bin/bijective-verify-venue".to_string(),
        arguments: vec![
            "/var/lib/bijective/store".to_string(),
            "{instance}".to_string(),
            "/etc/bijective/profiles/{instance}.json".to_string(),
            "BTCUSDT".to_string(),
            "-".to_string(),
        ],
        writes: vec!["/var/lib/bijective/store".to_string()],
        on_calendar: "daily".to_string(),
        instances: vec!["a-venue".to_string()],
        stop_timeout_seconds: 30,
    }
}

fn sound() -> Deployment {
    let mut control = program("control-engine", ProgramClass::Core);
    control.after = vec!["data-engine".to_string()];
    deployment(vec![
        program("data-engine", ProgramClass::Core),
        control,
        program("operational-host", ProgramClass::Operational),
    ])
}

#[test]
fn a_sound_manifest_validates() {
    assert!(sound().validate().is_ok());
}

#[test]
fn a_manifest_from_another_build_is_refused() {
    let mut d = sound();
    d.version = 99;
    assert_eq!(d.validate(), Err(ManifestError::VersionUnknown { found: 99 }));
}

#[test]
fn a_manifest_that_installs_nothing_is_refused() {
    assert_eq!(deployment(vec![]).validate(), Err(ManifestError::NoPrograms));
}

#[test]
fn a_name_declared_twice_is_refused() {
    let d = deployment(vec![
        program("data-engine", ProgramClass::Core),
        program("data-engine", ProgramClass::Core),
    ]);
    assert_eq!(d.validate(), Err(ManifestError::DuplicateName { name: "data-engine".into() }));
}

/// An unknown `After=` is accepted by the init system and the ordering silently stops existing.
#[test]
fn an_ordering_naming_a_program_that_is_not_installed_is_refused() {
    let mut p = program("control-engine", ProgramClass::Core);
    p.after = vec!["a-program-that-does-not-exist".to_string()];
    let d = deployment(vec![p]);

    assert_eq!(
        d.validate(),
        Err(ManifestError::DanglingOrder {
            program: "control-engine".into(),
            after: "a-program-that-does-not-exist".into(),
        })
    );
}

#[test]
fn a_program_ordered_after_itself_is_refused() {
    let mut p = program("data-engine", ProgramClass::Core);
    p.after = vec!["data-engine".to_string()];
    assert_eq!(
        deployment(vec![p]).validate(),
        Err(ManifestError::OrderingCycle { program: "data-engine".into() })
    );
}

/// A cycle through a third unit: the init system drops an edge it does not name.
#[test]
fn an_ordering_cycle_through_another_program_is_refused() {
    let mut a = program("a", ProgramClass::Core);
    let mut b = program("b", ProgramClass::Core);
    let mut c = program("c", ProgramClass::Core);
    a.after = vec!["b".into()];
    b.after = vec!["c".into()];
    c.after = vec!["a".into()];

    assert!(matches!(
        deployment(vec![a, b, c]).validate(),
        Err(ManifestError::OrderingCycle { .. })
    ));
}

#[test]
fn a_zero_resource_bound_is_refused_rather_than_read_as_unlimited() {
    let mut p = program("data-engine", ProgramClass::Core);
    p.memory_max_mb = 0;
    assert_eq!(
        deployment(vec![p]).validate(),
        Err(ManifestError::ZeroBound { program: "data-engine".into(), field: "memory_max_mb" })
    );
}

#[test]
fn an_empty_binary_path_is_refused() {
    let mut p = program("data-engine", ProgramClass::Core);
    p.binary = "  ".to_string();
    assert!(matches!(
        deployment(vec![p]).validate(),
        Err(ManifestError::EmptyField { field: "binary", .. })
    ));
}

/// A manifest of only operational units installs nothing that runs alone.
#[test]
fn a_manifest_with_no_core_program_is_refused() {
    let d = deployment(vec![program("operational-host", ProgramClass::Operational)]);
    assert_eq!(d.validate(), Err(ManifestError::NoCoreProgram));
}

/// The deployment kit acceptance criterion 2. Everything else in D2 rests on this.
#[test]
fn two_runs_over_one_manifest_produce_identical_bytes() {
    let d = sound();
    assert_eq!(generate(&d), generate(&d));
    assert_eq!(generate(&d).concatenated(), generate(&d).concatenated());
}

/// The manifest is a list, and a list has an order a human can change without meaning to.
#[test]
fn reordering_the_manifest_does_not_change_the_units() {
    let first = sound();
    let mut second = sound();
    second.programs.reverse();

    assert_eq!(generate(&first).units, generate(&second).units);
    assert_eq!(generate(&first).enable, generate(&second).enable);
}

/// An operational unit is installed and not enabled.
#[test]
fn an_operational_program_is_installed_and_not_enabled() {
    let g = generate(&sound());

    assert!(g.units.contains_key("bijective-operational-host.service"));
    assert!(!g.enable.contains(&"bijective-operational-host.service".to_string()));
    assert!(g.enable.contains(&"bijective-data-engine.service".to_string()));
}

/// Ordering without need lets the init system start a unit whose dependency has failed.
#[test]
fn an_ordering_is_emitted_as_both_after_and_requires() {
    let g = generate(&sound());
    let control = &g.units["bijective-control-engine.service"];

    assert!(control.contains("After=bijective-data-engine.service"));
    assert!(control.contains("Requires=bijective-data-engine.service"));
}

#[test]
fn the_resource_bound_and_stop_timeout_reach_the_unit() {
    let g = generate(&sound());
    let data = &g.units["bijective-data-engine.service"];

    assert!(data.contains("MemoryMax=512M"));
    assert!(data.contains("TimeoutStopSec=30"));
    assert!(data.contains("NoNewPrivileges=true"));
}

/// Without the marker nothing in the file says that editing it will be overwritten.
#[test]
fn every_unit_says_it_was_generated() {
    for body in generate(&sound()).units.values() {
        assert!(body.contains("X-Bijective-Generated=true"));
    }
}

fn artefact_tree(name: &str) -> PathBuf {
    let root = scratch(name);
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(root.join("bin/bijective-data-engine"), b"a binary, for the purposes of a hash")
        .unwrap();
    std::fs::write(root.join("manifest.json"), b"{}").unwrap();
    root
}

#[test]
fn an_intact_artefact_verifies() {
    let root = artefact_tree("intact");
    let sums = checksums_for(&root, "0.1.0").unwrap();
    assert_eq!(verify_artefact(&root, &sums).unwrap(), 2);
}

/// The deployment kit row one: the install refuses a mismatch. Not a warning.
#[test]
fn a_tampered_file_is_refused() {
    let root = artefact_tree("tampered");
    let sums = checksums_for(&root, "0.1.0").unwrap();
    std::fs::write(root.join("bin/bijective-data-engine"), b"something else entirely").unwrap();

    assert!(matches!(verify_artefact(&root, &sums), Err(ArtefactError::Mismatch { .. })));
}

#[test]
fn a_missing_file_is_refused() {
    let root = artefact_tree("missing");
    let sums = checksums_for(&root, "0.1.0").unwrap();
    std::fs::remove_file(root.join("manifest.json")).unwrap();

    assert!(matches!(verify_artefact(&root, &sums), Err(ArtefactError::Missing { .. })));
}

/// A manifest that only checks what it names permits anything to be added to a release.
#[test]
fn a_file_the_manifest_does_not_name_is_refused() {
    let root = artefact_tree("extra");
    let sums = checksums_for(&root, "0.1.0").unwrap();
    std::fs::write(root.join("bin/an-unexpected-binary"), b"where did this come from").unwrap();

    match verify_artefact(&root, &sums) {
        Err(ArtefactError::Unlisted { path }) => assert!(path.contains("an-unexpected-binary")),
        other => panic!("an unlisted file was accepted: {other:?}"),
    }
}

#[test]
fn the_checksum_manifest_does_not_have_to_list_itself() {
    let root = artefact_tree("selfref");
    let sums = checksums_for(&root, "0.1.0").unwrap();
    std::fs::write(root.join(CHECKSUMS_FILE), serde_json::to_vec(&sums).unwrap()).unwrap();

    assert!(verify_artefact(&root, &sums).is_ok());
}

#[test]
fn an_empty_checksum_manifest_is_refused_rather_than_vacuously_passing() {
    let root = artefact_tree("empty-sums");
    let mut sums = checksums_for(&root, "0.1.0").unwrap();
    sums.files.clear();

    assert_eq!(verify_artefact(&root, &sums), Err(ArtefactError::NoFiles));
}

use crate::sequencer::{NodeState, assess_node};
use bijective_common::readiness::{Health, Surface, document::MAX_AGE_NS};

const NOW: i128 = 1_700_000_000_000_000_000;

fn run_dir(name: &str) -> PathBuf {
    scratch(&format!("run-{name}"))
}

fn report(dir: &Path, program: &str, health: Health, detail: &str, at_ns: i128) {
    Surface::new(dir.join(format!("{program}.json")), program)
        .report(health, detail, at_ns)
        .unwrap();
}

#[test]
fn a_node_with_nothing_reporting_is_down() {
    let dir = run_dir("down");
    let node = assess_node(&sound(), &dir, NOW);

    assert_eq!(node.state, NodeState::Down);
    assert!(node.because.contains("has not been started"));
}

#[test]
fn a_core_program_still_replaying_holds_the_node_at_starting() {
    let dir = run_dir("starting");
    report(&dir, "data-engine", Health::Ready, "ingesting", NOW);
    report(&dir, "control-engine", Health::Replaying, "limit state at 40%", NOW);

    let node = assess_node(&sound(), &dir, NOW);
    assert_eq!(node.state, NodeState::Starting);
    assert!(node.because.contains("control-engine"), "the aggregate must name the holdout");
    assert!(node.because.contains("40%"), "and carry its own words");
}

#[test]
fn every_core_program_ready_makes_the_node_ready() {
    let dir = run_dir("ready");
    report(&dir, "data-engine", Health::Ready, "ingesting", NOW);
    report(&dir, "control-engine", Health::Ready, "verified; trading halted", NOW);

    let node = assess_node(&sound(), &dir, NOW);
    assert_eq!(node.state, NodeState::Ready);
    assert!(node.because.contains("trading is a separate state"));
}

/// Reporting the softer of two answers is how a fault waits for someone to notice it.
#[test]
fn a_failure_dominates_a_program_that_is_merely_absent() {
    let dir = run_dir("failure-dominates");
    report(&dir, "data-engine", Health::Failed, "store refused to open", NOW);
    // control-engine reports nothing at all.

    let node = assess_node(&sound(), &dir, NOW);
    assert_eq!(node.state, NodeState::Failed);
    assert!(node.because.contains("data-engine"));
    assert!(node.because.contains("store refused"));
}

/// The core-runs-alone rule at the level an operator reads: the core is ready with the switchable unit stopped.
#[test]
fn an_operational_program_that_is_not_running_does_not_hold_the_node_back() {
    let dir = run_dir("optin-absent");
    report(&dir, "data-engine", Health::Ready, "ingesting", NOW);
    report(&dir, "control-engine", Health::Ready, "verified", NOW);

    let node = assess_node(&sound(), &dir, NOW);
    assert_eq!(node.state, NodeState::Ready);
    assert!(node.degradations().is_empty());
}

/// Not-ready would be wrong and silent would be worse: continuity checks could stop unnoticed.
#[test]
fn an_operational_program_that_failed_is_surfaced_without_making_the_node_unready() {
    let dir = run_dir("optin-failed");
    report(&dir, "data-engine", Health::Ready, "ingesting", NOW);
    report(&dir, "control-engine", Health::Ready, "verified", NOW);
    report(&dir, "operational-host", Health::Failed, "2 segment(s) not clean of 5", NOW);

    let node = assess_node(&sound(), &dir, NOW);
    assert_eq!(node.state, NodeState::Ready);

    let degraded = node.degradations();
    assert_eq!(degraded.len(), 1);
    assert!(degraded[0].detail.contains("not clean"));
}

/// A program that died leaves its last claim of Ready on disk. The node must not accept it.
#[test]
fn a_core_program_that_stopped_reporting_stops_the_node_being_ready() {
    let dir = run_dir("stale");
    report(&dir, "data-engine", Health::Ready, "ingesting", NOW);
    report(&dir, "control-engine", Health::Ready, "verified", NOW);

    let later = NOW + MAX_AGE_NS + 1;
    let node = assess_node(&sound(), &dir, later);

    assert_eq!(node.state, NodeState::Failed);
    assert!(
        node.because.contains("stopped reporting"),
        "an operator sent to look at a start problem will not find a crash: {}",
        node.because
    );
}

/// The two ways of having no claim are different facts and must not read alike.
#[test]
fn never_reported_and_stopped_reporting_are_told_apart() {
    let never = run_dir("never");
    let stopped = run_dir("stopped");
    report(&stopped, "data-engine", Health::Ready, "ingesting", NOW);
    report(&stopped, "control-engine", Health::Ready, "verified", NOW);

    let a = assess_node(&sound(), &never, NOW);
    let b = assess_node(&sound(), &stopped, NOW + MAX_AGE_NS + 1);

    assert_eq!(a.state, NodeState::Down);
    assert_eq!(b.state, NodeState::Failed);
    assert_ne!(a.because, b.because);
}

#[test]
fn every_state_explains_itself_and_names_every_program() {
    let dir = run_dir("explains");
    report(&dir, "data-engine", Health::Starting, "opening the store", NOW);

    let node = assess_node(&sound(), &dir, NOW);
    assert!(!node.because.is_empty());
    assert_eq!(node.programs.len(), 3, "a program that is silent is still listed");
    assert!(node.programs.iter().all(|p| !p.detail.is_empty()));
}

use crate::notices::{
    Attested, NoticesError, distinct_licences, elected, parse_attestation, render,
};

fn pkg(name: &str, licence: &str) -> Attested {
    Attested { name: name.into(), version: "1.0.0".into(), licence: licence.into() }
}

#[test]
fn a_single_licence_elects_itself() {
    assert_eq!(elected("MIT"), vec!["MIT"]);
    assert_eq!(elected("Apache-2.0"), vec!["Apache-2.0"]);
}

/// Election must be deterministic, or the generated file is not reproducible.
#[test]
fn a_choice_elects_the_same_licence_every_time() {
    for _ in 0..5 {
        assert_eq!(elected("MIT OR Apache-2.0"), vec!["MIT"]);
        assert_eq!(
            elected("Apache-2.0 OR MIT"),
            vec!["MIT"],
            "order in the expression is irrelevant"
        );
        assert_eq!(elected("Unlicense/MIT"), vec!["MIT"], "the slash form is a choice too");
    }
}

/// No copyleft licence is elected when a permissive option is offered.
#[test]
fn a_copyleft_option_is_never_elected_when_a_permissive_one_is_offered() {
    assert_eq!(elected("MIT OR Apache-2.0 OR LGPL-2.1-or-later"), vec!["MIT"]);
    assert!(
        !distinct_licences(&[pkg("r-efi", "MIT OR Apache-2.0 OR LGPL-2.1-or-later")])
            .contains(&"LGPL-2.1-or-later".to_string())
    );
}

/// `(A OR B) AND C` elects one of A and B and also complies with C.
#[test]
fn a_conjunction_of_choices_keeps_the_mandatory_term() {
    let mut got = elected("(MIT OR Apache-2.0) AND Unicode-3.0");
    got.sort();
    assert_eq!(got, vec!["MIT", "Unicode-3.0"]);
}

/// A package whose every option is declined fails rather than being left out.
#[test]
fn a_package_offering_nothing_we_elect_refuses_the_whole_file() {
    let err = render(&[pkg("something", "GPL-3.0-only")], &Default::default()).unwrap_err();
    assert!(matches!(err, NoticesError::NoElection { .. }), "got {err:?}");
    assert!(err.to_string().contains("GPL-3.0-only"));
}

/// A licence named with no text found fails. Omitting it would look like discharge.
#[test]
fn a_missing_licence_text_refuses_rather_than_omitting_it() {
    let err = render(&[pkg("a-crate", "MIT")], &Default::default()).unwrap_err();
    assert_eq!(err, NoticesError::TextMissing { licence: "MIT".into() });
}

#[test]
fn the_attestation_is_parsed_and_sorted() {
    let raw = "# a comment\nzlib\t1.0.0\tZlib\nadler\t2.0.0\tMIT\n\n";
    let parsed = parse_attestation(raw).unwrap();

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].name, "adler", "sorted, so the output is reproducible");
}

#[test]
fn a_malformed_attestation_line_is_refused_with_its_number() {
    assert_eq!(
        parse_attestation("good\t1.0\tMIT\nbad line with no tabs\n").unwrap_err(),
        NoticesError::Malformed { line: 2 }
    );
    assert_eq!(parse_attestation("# only comments\n").unwrap_err(), NoticesError::Empty);
}

/// No timestamp, host name or varying order, so the build check is a diff.
#[test]
fn two_renders_of_one_attestation_are_identical() {
    let packages = vec![pkg("b-crate", "MIT"), pkg("a-crate", "MIT OR Apache-2.0")];
    let mut texts = std::collections::BTreeMap::new();
    texts.insert("MIT".to_string(), "the MIT text".to_string());

    let first = render(&packages, &texts).unwrap();
    assert_eq!(first, render(&packages, &texts).unwrap());
    assert!(first.contains("| a-crate | 1.0.0 | MIT OR Apache-2.0 | MIT |"));
}

/// Only a program that declares it gets a delegated cgroup subtree.
#[test]
fn a_delegated_subtree_goes_only_to_the_program_that_declared_it() {
    let mut supervisor = program("strategy-engine", ProgramClass::Core);
    supervisor.delegates_cgroup = true;
    let plain = program("data-engine", ProgramClass::Core);

    let generated = generate(&deployment(vec![supervisor, plain]));

    let supervising = &generated.units["bijective-strategy-engine.service"];
    let other = &generated.units["bijective-data-engine.service"];

    assert!(
        supervising.contains("Delegate=yes"),
        "without it the supervisor cannot move a run into a cgroup and refuses to start"
    );
    assert!(!other.contains("Delegate="));
}

/// A program writes only the paths it declared. The unit mounts the filesystem read-only.
#[test]
fn a_unit_grants_exactly_the_paths_its_program_declared() {
    let mut writing = program("data-engine", ProgramClass::Core);
    writing.writes = vec!["/var/lib/bijective/store".into(), "/run/bijective".into()];
    let silent = program("operational-host", ProgramClass::Operational);

    let generated = generate(&deployment(vec![writing, silent]));
    let unit = &generated.units["bijective-data-engine.service"];

    assert!(
        unit.contains("ReadWritePaths=/run/bijective /var/lib/bijective/store"),
        "the paths were not granted, or not in a fixed order: {unit}"
    );
    // Sorted, so two runs over one manifest give the same bytes.
    assert!(
        !generated.units["bijective-operational-host.service"].contains("ReadWritePaths="),
        "a program that declared nothing was granted something"
    );
}

/// A credential is named by where it is, never by what it is.
#[test]
fn a_unit_names_where_a_credential_is_and_never_what_it_is() {
    let mut engine = program("control-engine", ProgramClass::Core);
    engine.credentials = vec!["bijective-init:/etc/bijective/credentials/bijective-init".into()];
    let plain = program("operational-host", ProgramClass::Operational);

    let generated = generate(&deployment(vec![engine, plain]));
    let unit = &generated.units["bijective-control-engine.service"];

    assert!(
        unit.contains("LoadCredential=bijective-init:/etc/bijective/credentials/bijective-init"),
        "the init system was not told where the credential is: {unit}"
    );
    assert!(
        !generated.units["bijective-operational-host.service"].contains("LoadCredential="),
        "a program that declared no credential was given one"
    );
}

/// The supervisor is installed and not started, and is the only one with a subtree.
#[test]
fn the_supervisor_is_installed_and_left_for_an_operator_to_start() {
    let mut supervisor = program("strategy-engine", ProgramClass::Operational);
    supervisor.delegates_cgroup = true;

    let generated = generate(&deployment(vec![supervisor]));

    assert!(generated.units.contains_key("bijective-strategy-engine.service"));
    assert!(
        !generated.enable.contains(&"bijective-strategy-engine.service".to_string()),
        "a session would start on boot, at an unset stage"
    );
}

/// A check becomes a template and a timer, and the timer is what gets enabled.
#[test]
fn a_scheduled_check_is_a_template_and_a_timer() {
    let mut d = sound();
    d.checks = vec![check()];

    let g = generate(&d);

    assert!(g.units.contains_key("bijective-assurance@.service"));
    assert!(g.units.contains_key("bijective-assurance@.timer"));
    assert!(
        g.enable.contains(&"bijective-assurance@a-venue.timer".to_string()),
        "the timer is enabled per instance, not the service: {:?}",
        g.enable
    );
    assert!(
        !g.enable.iter().any(|u| u.ends_with("@a-venue.service")),
        "enabling the service would run it once at boot and never again"
    );
}

/// The instance token becomes the init system's own substitution, in every argument that carries it.
#[test]
fn the_instance_token_becomes_the_units_instance() {
    let mut d = sound();
    d.checks = vec![check()];

    let service = generate(&d).units["bijective-assurance@.service"].clone();

    assert!(service.contains("/etc/bijective/profiles/%i.json"), "{service}");
    assert!(!service.contains("{instance}"), "the token must not survive into the unit: {service}");
    assert!(service.contains("Type=oneshot"), "a check runs and exits: {service}");
    assert!(!service.contains("Restart="), "a restarted check hides its own failure: {service}");
}

/// A node that was off through the window still checks before it is trusted.
#[test]
fn a_missed_run_is_caught_up_rather_than_skipped() {
    let mut d = sound();
    d.checks = vec![check()];

    let timer = generate(&d).units["bijective-assurance@.timer"].clone();

    assert!(timer.contains("OnCalendar=daily"), "{timer}");
    assert!(
        timer.contains("Persistent=true"),
        "a skipped run ages the conclusion silently: {timer}"
    );
    assert!(timer.contains("Unit=bijective-assurance@%i.service"), "{timer}");
}

/// A check nothing would ever run is refused rather than installed.
#[test]
fn a_check_with_no_calendar_is_refused() {
    let mut d = sound();
    let mut c = check();
    c.on_calendar = "  ".to_string();
    d.checks = vec![c];

    assert_eq!(d.validate(), Err(ManifestError::NoCalendar { check: "assurance".to_string() }));
}

/// An instance the init system could not carry would become a different unit, or none.
#[test]
fn an_instance_the_init_system_could_not_carry_is_refused() {
    for bad in ["", "a venue", "a/venue"] {
        let mut d = sound();
        let mut c = check();
        c.instances = vec![bad.to_string()];
        d.checks = vec![c];

        assert!(
            matches!(d.validate(), Err(ManifestError::UnusableInstance { .. })),
            "{bad:?} must be refused"
        );
    }
}

/// A check that shares a name with a program would collide in the unit set.
#[test]
fn a_check_named_after_a_program_is_refused() {
    let mut d = sound();
    let mut c = check();
    c.name = "data-engine".to_string();
    d.checks = vec![c];

    assert_eq!(
        d.validate(),
        Err(ManifestError::DuplicateCheck { name: "data-engine".to_string() })
    );
}

/// Determinism holds with checks in the manifest, which is what makes a hand edit detectable.
#[test]
fn a_manifest_with_checks_still_generates_the_same_bytes_twice() {
    let mut d = sound();
    d.checks = vec![check()];

    assert_eq!(generate(&d).concatenated(), generate(&d).concatenated());
}
