#![cfg(feature = "mob")]

use std::process::{Command, Output};

const ACTIVE_DOCS_NAV: &str = include_str!("../../docs/docs.json");
const ACTIVE_MOBPACK_DOCS: &[(&str, &str)] = &[
    (
        "guides/mobpack",
        include_str!("../../docs/guides/mobpack.mdx"),
    ),
    ("guides/mobs", include_str!("../../docs/guides/mobs.mdx")),
    (
        "examples/mobpack",
        include_str!("../../docs/examples/mobpack.mdx"),
    ),
    (
        "examples/wasm",
        include_str!("../../docs/examples/wasm.mdx"),
    ),
    ("cli/commands", include_str!("../../docs/cli/commands.mdx")),
    (
        "reference/mob-architecture",
        include_str!("../../docs/reference/mob-architecture.mdx"),
    ),
];

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn fenced_block<'a>(section: &'a str, language: &str) -> Result<&'a str, String> {
    let opening = format!("```{language}\n");
    section
        .split_once(&opening)
        .and_then(|(_, rest)| rest.split_once("\n```").map(|(body, _)| body))
        .ok_or_else(|| format!("missing {language} block in embedded mobpack help"))
}

fn assert_local_pack_commands_use_permissive(source: &str, body: &str) {
    let joined_commands = body.replace("\\\n", " ");
    for line in joined_commands.lines().map(str::trim) {
        let concrete_pack_path = line.contains(".mobpack") && !line.contains('<');
        let verifies_pack = line.starts_with("rkat mob validate");
        let runs_pack = line.starts_with("rkat mob run");
        let builds_pack = line.starts_with("rkat mob web build");
        if concrete_pack_path && (verifies_pack || runs_pack || builds_pack) {
            assert!(
                line.contains("--trust-policy permissive"),
                "{source} has a default-strict local pack command: {line}"
            );
        }
    }
}

fn assert_active_mobpack_doc_contract(route: &str, body: &str) {
    assert!(
        ACTIVE_DOCS_NAV.contains(&format!("\"{route}\"")),
        "contract test must cover only an active docs route: {route}"
    );

    let collapsed = body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    for stale in [
        "the web build compiles",
        "committed `sdks/web/wasm`",
        "--wasm ./sdks/web/wasm",
        "--wasm sdks/web/wasm",
        "`wasm-pack` availability for `mob web build`",
    ] {
        assert!(
            !collapsed.contains(stale),
            "active doc {route} restored stale contract: {stale}"
        );
    }

    assert_local_pack_commands_use_permissive(&format!("active doc {route}"), body);

    if collapsed.contains("rkat mob web build") {
        assert!(
            collapsed.contains("copies")
                && collapsed.contains("prebuilt")
                && collapsed.contains("does not compile"),
            "active doc {route} must say web build copies prebuilt artifacts without compiling wasm"
        );
    }
}

#[test]
fn embedded_help_mobpack_examples_match_typed_authoring_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    for (route, body) in ACTIVE_MOBPACK_DOCS {
        assert_active_mobpack_doc_contract(route, body);
    }
    assert_local_pack_commands_use_permissive(
        "embedded references/mobs.md",
        meerkat::help::MEERKAT_PLATFORM_MOBS_REFERENCE,
    );

    let section = meerkat::help::MEERKAT_PLATFORM_SKILL_BODY
        .split_once("### Mobpack + web build quick paths")
        .map(|(_, section)| section)
        .ok_or("embedded help must contain the mobpack quick-path section")?;

    let manifest: meerkat_mob_pack::manifest::MobpackManifest =
        toml::from_str(fenced_block(section, "toml")?)?;
    assert_eq!(manifest.mobpack.name, "review-team");
    assert_eq!(manifest.mobpack.version, "1.0.0");

    let definition: meerkat_mob::MobDefinition =
        serde_json::from_str(fenced_block(section, "json")?)?;
    assert_eq!(definition.id.as_str(), "review-team");
    assert_eq!(definition.profiles.len(), 2);
    assert!(definition.profiles.values().all(|binding| {
        binding
            .as_inline()
            .is_some_and(|profile| profile.tools.comms)
    }));
    assert_eq!(definition.wiring.role_wiring.len(), 1);
    assert_eq!(definition.flows.len(), 1);
    let main_flow = definition
        .flows
        .values()
        .next()
        .ok_or("definition must contain the main flow")?;
    assert_eq!(main_flow.steps.len(), 1);

    let command_block = fenced_block(section, "bash")?;
    assert!(
        command_block
            .contains("mob validate ./dist/release-triage.mobpack --trust-policy permissive"),
        "a locally signed pack is not trusted merely because pack embedded its key"
    );
    assert!(
        command_block.contains(
            "rkat mob run ./dist/release-triage.mobpack --flow main --trust-policy permissive"
        ),
        "mob run must exercise automatic flat-step role provisioning"
    );
    assert!(
        command_block.contains("--wasm <PKG_DIR|name_bg.wasm> --trust-policy permissive"),
        "web build must receive both its prebuilt runtime and the honest local trust posture"
    );
    assert!(
        meerkat::help::MEERKAT_CLI_REFERENCE_SKILL_BODY
            .contains("rkat mob web build <pack> -o <dir> --wasm <PKG_DIR|name_bg.wasm>"),
        "the exact CLI authority must make --wasm required"
    );
    let normalized_section = section.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized_section.contains("automatically provision one turn-driven target")
            && normalized_section.contains("Pre-spawn through a host or SDK only when you need"),
        "help must explain automatic flow provisioning and the explicit pre-spawn boundary"
    );

    // Exercise the documented artifact path through the shipped CLI, rather
    // than accepting examples merely because their TOML and JSON deserialize.
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("mobs").join("release-triage");
    let dist = temp.path().join("dist");
    std::fs::create_dir_all(&source)?;
    std::fs::create_dir_all(&dist)?;
    std::fs::write(source.join("manifest.toml"), fenced_block(section, "toml")?)?;
    std::fs::write(
        source.join("definition.json"),
        fenced_block(section, "json")?,
    )?;
    let signing_key = temp.path().join("release.key");
    std::fs::write(
        &signing_key,
        "0707070707070707070707070707070707070707070707070707070707070707\n",
    )?;
    let pack = dist.join("release-triage.mobpack");
    let rkat = env!("CARGO_BIN_EXE_rkat");

    let empty_path = temp.path().join("empty-path");
    std::fs::create_dir_all(&empty_path)?;
    let doctor = Command::new(rkat)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("PATH", &empty_path)
        .arg("doctor")
        .output()?;
    let doctor_output = output_text(&doctor);
    assert!(
        doctor_output.contains(
            "one way to generate the prebuilt artifacts required by `rkat mob web build --wasm`"
        ) && doctor_output.contains("the build command does not invoke wasm-pack"),
        "doctor must describe wasm-pack as an optional artifact generator: {doctor_output}"
    );
    assert!(
        !doctor_output.contains("needed for `rkat mob web build`"),
        "doctor restored the false direct dependency claim: {doctor_output}"
    );

    let packed = Command::new(rkat)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env_remove("RKAT_TRUST_POLICY")
        .args(["mob", "pack"])
        .arg(&source)
        .args(["-o"])
        .arg(&pack)
        .args(["--sign"])
        .arg(&signing_key)
        .args(["--signer-id", "team@example.com"])
        .output()?;
    assert!(
        packed.status.success(),
        "mob pack failed: {}",
        output_text(&packed)
    );

    let inspected = Command::new(rkat)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .arg("mob")
        .arg("inspect")
        .arg(&pack)
        .output()?;
    assert!(
        inspected.status.success(),
        "mob inspect failed: {}",
        output_text(&inspected)
    );

    let permissive = Command::new(rkat)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env_remove("RKAT_TRUST_POLICY")
        .arg("mob")
        .arg("validate")
        .arg(&pack)
        .args(["--trust-policy", "permissive"])
        .output()?;
    assert!(
        permissive.status.success(),
        "permissive mob validate failed: {}",
        output_text(&permissive)
    );
    assert!(
        String::from_utf8_lossy(&permissive.stdout)
            .contains("signature valid but signer is unknown in permissive mode"),
        "permissive validation must surface unknown-signer warning: {}",
        output_text(&permissive)
    );

    let strict = Command::new(rkat)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env_remove("RKAT_TRUST_POLICY")
        .arg("mob")
        .arg("validate")
        .arg(&pack)
        .args(["--trust-policy", "strict"])
        .output()?;
    assert!(
        !strict.status.success()
            && String::from_utf8_lossy(&strict.stderr).contains("unknown signer"),
        "strict validation must reject the uninstalled signer: {}",
        output_text(&strict)
    );

    let wasm_pkg = temp.path().join("wasm-pkg");
    std::fs::create_dir_all(&wasm_pkg)?;
    let wasm_bytes = b"\0asm\x01\0\0\0";
    std::fs::write(wasm_pkg.join("real_bg.wasm"), wasm_bytes)?;
    std::fs::write(
        wasm_pkg.join("real.js"),
        "export default function init() {}\n",
    )?;
    let web_out = dist.join("release-triage-web");
    let web_build = Command::new(rkat)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env_remove("RKAT_TRUST_POLICY")
        .arg("mob")
        .arg("web")
        .arg("build")
        .arg(&pack)
        .args(["-o"])
        .arg(&web_out)
        .arg("--wasm")
        .arg(&wasm_pkg)
        .args(["--trust-policy", "permissive"])
        .output()?;
    assert!(
        web_build.status.success(),
        "permissive mob web build failed: {}",
        output_text(&web_build)
    );
    assert_eq!(std::fs::read(web_out.join("real_bg.wasm"))?, wasm_bytes);
    assert_eq!(
        std::fs::read_to_string(web_out.join("real.js"))?,
        "export default function init() {}\n"
    );
    for required in [
        "mobpack.bin",
        "manifest.web.toml",
        "meerkat-bootstrap.js",
        "index.html",
    ] {
        assert!(
            web_out.join(required).is_file(),
            "web build must emit {required}"
        );
    }

    Ok(())
}
