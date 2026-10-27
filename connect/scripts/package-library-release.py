#!/usr/bin/env python3
"""Package exact accepted consumer inputs with Python 3.11+; no installation or publication."""

from __future__ import annotations

import argparse
import copy
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import posixpath
import re
import stat
import tarfile


ROOT = Path(__file__).resolve().parents[1]
EDITION = "20261009-library-support-selection"
MAX_FILE_BYTES = 64 * 1024 * 1024
MAX_TOTAL_BYTES = 256 * 1024 * 1024
MAX_ENTRIES = 4096
MAX_DONOR_ENTRIES = 5000
HEX = re.compile(r"[0-9a-f]{64}")
SAFE_PART = re.compile(r"[A-Za-z0-9@_+.,() -]+")

# Build-time input pins, not a replacement for either consumer's runtime manifest.
TARGETS = {
    "macos-arm64": {
        "setup_assets": {
            "setup/static/index.html": ("sdk/consumer-setup/static/index.html", "2eba9823115bdb3125fbba016eacea3d19a8aed598e3573a1f776eba26ed5496"),
            "setup/static/app.js": ("sdk/consumer-setup/static/app.js", "234ac25db18262f50667c3e1e186dc3816935242deae4cc729346dfbb21d67f6"),
            "setup/static/form.js": ("sdk/consumer-setup/static/form.js", "548814a95768b83e62c6f2d9e3ce1295195f3be1bb82109281caab8d178781db"),
            "setup/static/style.css": ("sdk/consumer-setup/static/style.css", "3141b6114a6123cbf35ffc516fbf37cb0de614536d2fb50fe65b77f7f01dbb82"),
        },
        "setup_backend": ("sdk/consumer-setup/backend.py", "ec796d68e18e92ae6e3ca6042ca1ef4521480293c264266af2b7d7c6b1ef4ef9"),
        "setup_installer": ("docs/verification/20261003-linux-github-setup/packaging/retained-macos/setup/install-library-consumer.py", "ed81e546d0180986d6f80f4cdb1c105fd124d4e3fdba0e21d8eb3d66c757de16"),
        "setup_adapter": ("sdk/consumer-setup/mcp_adapter.py", "88798c5229fc77498f07efd321a98ef64398943eb220fc4b1397a52137eecba2"),
        "bundle": "dist/local-consumers-20261009-library-support-selection-macos",
        "contents": "f1e5a8ff7c54b5d8aec8fee958a6f52f1f75fa53911d66794efbc544ac89b06c",
        "sums": "9df6a9f9ee6ebefeedafd7f45a1188c13c28616158b3a9cef6d7c385d5c79be0",
        "guide": "docs/adoption/release/macOS.md",
        "guide_sha256": "ea5e5cd8aee852afd60175b2599a4cd4dbfa67f5b0968c6041d559cca983fe1a",
        "official": {
            "bundle": "dist/official-consumers-20261003",
            "contents": "2f8663b84bd9b1b8a11d9ec51ef82af6a93877b5f12934b5047fcb4bce2850d7",
            "sums": "0f9eb0a0b09f281acd6f8c6ce72f5a66bad2802f937140129fd977514f8646a3",
            "components": ("github", "playwright"),
        },
    },
    "linux-x86_64": {
        "edition": "20261008-linux-mcpb",
        "setup_assets": {
            "setup/static/index.html": ("sdk/consumer-setup/static/index.html", "2eba9823115bdb3125fbba016eacea3d19a8aed598e3573a1f776eba26ed5496"),
            "setup/static/app.js": ("sdk/consumer-setup/static/app.js", "234ac25db18262f50667c3e1e186dc3816935242deae4cc729346dfbb21d67f6"),
            "setup/static/form.js": ("sdk/consumer-setup/static/form.js", "548814a95768b83e62c6f2d9e3ce1295195f3be1bb82109281caab8d178781db"),
            "setup/static/style.css": ("sdk/consumer-setup/static/style.css", "3141b6114a6123cbf35ffc516fbf37cb0de614536d2fb50fe65b77f7f01dbb82"),
        },
        "setup_backend": ("sdk/consumer-setup/backend.py", "ec796d68e18e92ae6e3ca6042ca1ef4521480293c264266af2b7d7c6b1ef4ef9"),
        "setup_adapter": ("sdk/consumer-setup/mcp_adapter.py", "88798c5229fc77498f07efd321a98ef64398943eb220fc4b1397a52137eecba2"),
        "bundle": "dist/local-consumers-20261009-library-support-selection-linux",
        "contents": "379b653664b8a7cd277f8819f6a684d9845d33e56d1752522732c209e5f5e87f",
        "sums": "18c01634241a3ad739759217d7cb52540b04f1087305279f63f4a9aeebc54801",
        "guide": "docs/verification/20261008-linux-mcpb/linux-release.md",
        "guide_sha256": "20c89e582a0cbfec9ddd6e38b5fea4a11285952a878652490f1cbc274d616db7",
        "setup_installer": ("docs/verification/20261004-library-service-scope/packaging/linux-official-installer.py", "0432bda2033ab6e3cccee3ffa0cb03d569fe4d77754f2d67da49001a730d7a68"),
        "official": {
            "bundle": "dist/official-github-linux-20261003-setup",
            "contents": "e8c7beb7951829c5d1e401caa2c16d80bfddeb77f2fdfdc54d2144e115faa20c",
            "sums": "b8517d9bef28fb3376e3778a687718de1cb8e89695244a7dc2b41f0969baddf9",
            "components": ("github",),
        },
    },
}
SOURCES = {
    "LICENSE-MIT": ("LICENSE-MIT", "b45d1f632f83aeb359da8e2b8716ad72591ec7267a489fdda39989c618d65a0f"),
    "LICENSE-APACHE": ("LICENSE-APACHE", "3a0ffd02ca79c2a687921b0f5d8094f2f6a7058a4c01775d1351ea6514fcd180"),
    "setup/static/index.html": ("docs/verification/20261003-setup-reconciliation/retained-linux-setup/static/index.html", "7fc14da02ddf51a6db63fa173eaa92815ce2052c234bf615a3ad4d8ba7e177d0"),
    "setup/static/app.js": ("docs/verification/20261003-setup-reconciliation/retained-linux-setup/static/app.js", "2a9d78e40ff1b05f9baf7e7e77a633e265a80969e3c25259ade3f7fcfa477da2"),
    "setup/static/form.js": ("docs/verification/20261003-setup-reconciliation/retained-linux-setup/static/form.js", "a2856ddcb931d906c730a5e8c65f393483464e7922393502019e9a82b5600f91"),
    "setup/static/style.css": ("docs/verification/20261003-setup-reconciliation/retained-linux-setup/static/style.css", "fb78d2dc5e6ae3a9dd85cbf47b65c8793a9f613bf9708aa4ea6ddb618fdcf6f7"),
}
OFFICIAL_EXECUTABLE = "github/github-mcp-server"

# Exact maintainer associations, not name matching or inferred provider choice.
OFFICIAL_ROUTE_BINDINGS = {
    "macos-arm64": [
        {"identity": {"publisher": "meerkat-toolkit", "name": "github",
                      "version": "metadata-sha256:572a7abea50be2e725062434bcc4157a8750b9a7f3b69a369fb9c036ce0f8ba6"},
         "strategy_id": "selected-local-mcp", "family": "official-github",
         "profile": "selected-reads", "path": "github/github-mcp-server"},
        {"identity": {"publisher": "meerkat-toolkit", "name": "playwright",
                      "version": "metadata-sha256:6e626634427de6f87dfe932a3a8b9984aa8fd3db20d48f32a53868f7b7714ec0"},
         "strategy_id": "selected-local-mcp", "family": "official-playwright",
         "profile": "isolated-chrome", "path": "playwright/package-lock.json"},
    ],
    "linux-x86_64": [
        {"identity": {"publisher": "meerkat-toolkit", "name": "github",
                      "version": "metadata-sha256:572a7abea50be2e725062434bcc4157a8750b9a7f3b69a369fb9c036ce0f8ba6"},
         "strategy_id": "selected-local-mcp", "family": "official-github",
         "profile": "selected-reads", "path": "github/github-mcp-server"},
    ],
}

# Reuse accepted tools, not the donor's setup runtime or bootstrap environment.
PYTHON_KITS = {
    "macos-arm64": {
        "archive": "dist/agent-install-kit-20261004-acquisition-recovery/agent-install-kit-macos-arm64.tar.gz",
        "sha256": "6c3c6c7d47aefbbcbe1c6694b756011fb4ad94d645f6eb8d8ee9e9bb71b889b1",
        "root": "agent-install-kit-macos-arm64",
        "tool_updates": {
            "bin/toolkit-compose-plan": {
                "source": "docs/verification/20261006-compose-migration-delivery/author/macos-build/toolkit-compose-plan",
                "sha256": "7f2446f5de2c5103efcb35e8e8961adea3d00979ad12774991055714a7455db1",
                "record": "docs/verification/20261006-compose-migration-delivery/author/macos-build/release-binary.json",
            },
            "bin/toolkit-local-recipes": {
                "source": "dist/portable-json-20261006-macos-arm64/toolkit-local-recipes",
                "sha256": "97138b0cd99d16325351225abc51ef090f80d73ea608bac89a840bd4214fb913",
                "record": "docs/verification/20261006-portable-json-delivery/mac-recipes-release-binary.json",
            },
            "bin/toolkit-package-inspect": {
                "source": "dist/portable-json-20261006-macos-arm64/toolkit-package-inspect",
                "sha256": "fe122a50ea1c6921b1f5bb4e3151322e2e74019ede63cdb731140d91e411aa25",
                "record": "docs/verification/20261006-portable-json-delivery/mac-inspect-release-binary.json",
            },
        },
        "capabilities": ["agent-plugin-files", "mcpb-archive-inspection", "compose-local-execution"],
        "tools": {
            "bin/toolkit-compose-plan": "f561e627df9143469e8e48f5fb8289f90226dcfb153287adcb2ac89050800e21",
            "bin/install-library-consumer.py": "c04a86a9bbd9b6384be1c03bea29b1f816fe8965c070da34237e46ad31dc2481",
            "bin/toolkit-node-plan": "6d6ae91893bdde3f31874b9720879e4754781a43034be32a74ab93e474463407",
            "bin/toolkit-python-plan": "0436d6682899f33fc6eefb9246d49397a8f63c401017db261a68e4618b4e9f55",
            "bin/toolkit-package-inspect": "3cd41ad0152acd87084ee91b4ec5a51732cc5f5205637e62d825b8f30660c5c8",
            "bin/toolkit-local-recipes": "ebdfefe37d0eecb3b6b8f7197dc4223a60f3a03eacf3db3b9367ce545b595c76",
            "bin/toolkit-mcp-check.py": "6fac185c165013f269167397f677788c9d7d645b91227b5f9266d458a45fccfe",
            "bin/toolkit-mcpb-install": "9303107a60e73615ffc93145cd04d4600c3abc79b82c857d7bddaddaeac04763",
        },
        "provenance_trees": ("provenance/python/", "provenance/node/", "provenance/inspector/", "provenance/rust/", "provenance/current-setup/", "provenance/mcpb-setup/", "provenance/compose-acquisition/"),
        "provenance_files": (),
        "contracts": {
            "selected/contracts/LOCAL-RECIPES.md": "crates/toolkit-package/LOCAL-RECIPES.md",
            "selected/contracts/MCP-EXECUTION-CHECK.md": "docs/guides/mcp-execution-check.md",
            "selected/contracts/MCPB-INSTALL.md": "crates/toolkit-package/MCPB-INSTALL.md",
        },
        "sources": {
            'selected/provenance/compose-migration/source.json': ('docs/verification/20261006-compose-migration-delivery/author/provenance/source.json', '384b9b34a94f211aee2304704f600c0f198d99e6f94568446712e0865b77abb0'),
            'selected/provenance/compose-migration/source.tar.gz': ('docs/verification/20261006-compose-migration-delivery/author/provenance/source.tar.gz', '5763a04898122377ff8cc3746bb7f5a31b1bed120cfaa90c4368981af997c352'),
            'selected/provenance/compose-migration/macos-build.json': ('docs/verification/20261006-compose-migration-delivery/author/provenance/macos-build.json', 'eb75709ac80b3afec703d4c291417027da2457b4286ecd076da3cb23f122846b'),
            'selected/provenance/compose-migration/macos-dependencies.json': ('docs/verification/20261006-compose-migration-delivery/author/provenance/macos-dependencies.json', '96747c56cfed44ad30feddd69db2a0ebf3c511c07da31eaf71a66e81594b4e9d'),
            'selected/provenance/compose-migration/build-execution.json': ('docs/verification/20261006-compose-migration-delivery/author/macos-build/execution.json', 'b07aba7994bed0f9b9fafef54a86b870adc3fe22863f80ec725bb68fff9a4dd0'),
            'selected/provenance/compose-migration/build-preflight.json': ('docs/verification/20261006-compose-migration-delivery/author/macos-build/preflight.json', 'bd7a021f9b9e071204b84e33777c10b4e1530824123add16a7c4661abfb605b8'),
            'selected/provenance/compose-migration/build.stdout': ('docs/verification/20261006-compose-migration-delivery/author/macos-build/build.stdout', '311c0a3dc137de6fda1019326cc4c0ac8eb7ed3cfe91dc375c49066da0dbadf5'),
            'selected/provenance/compose-migration/build.stderr': ('docs/verification/20261006-compose-migration-delivery/author/macos-build/build.stderr', 'b8fc8132a5a81d9429b48c67625b5ca6b31b82bb35cd8ad349c1ef5b990aa6ee'),
            'selected/provenance/compose-retention/source.json': ('docs/verification/20261005-compose-retention-delivery/author/provenance/source.json', '39f4e89091e9b2cd83959880149c5f99b9e47726a08315ed86a6158fd41e31c2'),
            'selected/provenance/compose-retention/source.tar.gz': ('docs/verification/20261005-compose-retention-delivery/author/provenance/source.tar.gz', '8a56cbf81dfc86adf10579212e79b755f7f490a8fae35d6047fc151d6fee54cd'),
            'selected/provenance/compose-retention/macos-build.json': ('docs/verification/20261005-compose-retention-delivery/author/provenance/macos-build.json', '7165bb0b7e553ad35409d70ff6f8695a1fa5700d7b4776f4a1b37764e1d41c46'),
            'selected/provenance/compose-retention/macos-dependencies.json': ('docs/verification/20261005-compose-retention-delivery/author/provenance/macos-dependencies.json', 'a799eac1db585017a9ea6e8f64a02f99d1521581a7e64a78236a6d9ea45f71a4'),
            'selected/provenance/compose-retention/build-execution.json': ('docs/verification/20261005-compose-retention-delivery/author/macos-build/execution.json', '6ca3431403889e0a1b5f6f3506fa3cb814f8a26d78443ed1e711b1ea8c55f384'),
            'selected/provenance/compose-retention/build-preflight.json': ('docs/verification/20261005-compose-retention-delivery/author/macos-build/preflight.json', 'be9e2c8faec5fef88438904b33453ff6b7f799dc37b4fc39d2c736e533c27d2f'),
            'selected/provenance/compose-retention/build.stdout': ('docs/verification/20261005-compose-retention-delivery/author/macos-build/build.stdout', '899ee86d5dd441189f2f2cc4678fd04c5676182fba1dc45776f70156504ae500'),
            'selected/provenance/compose-retention/build.stderr': ('docs/verification/20261005-compose-retention-delivery/author/macos-build/build.stderr', 'e076a523870ad070031aa2c4fb680b957b31c781edba613c2298a8ab4d0617b2'),
            "selected/provenance/compose-successor/source.json": ("docs/verification/20261005-compose-successor-delivery/author/provenance-r2/source.json", "40a9a217c1c074d8a7ef63fe390e0b40355620763285272007bac177fe374ed5"),
            "selected/provenance/compose-successor/source.tar.gz": ("docs/verification/20261005-compose-successor-delivery/author/provenance-r2/source.tar.gz", "a507f155ae3271eab420dfebe229d58b6191b614057e8e9823bb9f1ab66e049f"),
            "selected/provenance/compose-successor/macos-dependencies.json": ("docs/verification/20261005-compose-successor-delivery/author/provenance-r2/macos-dependencies.json", "88e2c18f1925cafbb4e89286f58295dc47072dd3872ad89b434c67b7c801bd1e"),
            "selected/provenance/compose-successor/planner-build-2.json": ("docs/verification/20261005-compose-successor-delivery/author/provenance-r2/planner-build-2.json", "dac624a9d0aba6571640a971c6279e5c94fc29fb8eb23a5ee98fefd9f109cfd3"),
            "selected/provenance/compose-successor/planner-build-2.stdout": ("docs/verification/20261005-compose-successor-delivery/author/provenance-r2/planner-build-2.stdout", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            "selected/provenance/compose-successor/planner-build-2.stderr": ("docs/verification/20261005-compose-successor-delivery/author/provenance-r2/planner-build-2.stderr", "132680476661a9b0346b59b88e2488baca4a1bde619435b684736282f2f67766"),
            'selected/licenses/crates/serde_path_to_error-0.1.20/LICENSE-APACHE': ('docs/verification/20261005-package-diagnostics/notices/licenses/crates/serde_path_to_error-0.1.20/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
            'selected/licenses/crates/serde_path_to_error-0.1.20/LICENSE-MIT': ('docs/verification/20261005-package-diagnostics/notices/licenses/crates/serde_path_to_error-0.1.20/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
            'selected/provenance/crates/serde_path_to_error-0.1.20.crate': ('docs/verification/20261005-package-diagnostics/notices/provenance/crates/serde_path_to_error-0.1.20.crate', '10a9ff822e371bb5403e391ecd83e182e0e77ba7f6fe0160b795797109d1b457'),
            'selected/provenance/package-diagnostics/dependency-coverage.json': ('docs/verification/20261005-package-diagnostics/notices/coverage.json', '56ecca6d63bb472feae61367095a70a7237bd4629bde699cf21ac6f81b0996b8'),
            'selected/provenance/package-diagnostics/serde_path_to_error-0.1.20.source.json': ('docs/verification/20261005-package-diagnostics/notices/provenance/package-diagnostics/serde_path_to_error-0.1.20.source.json', '2f3144d4eba33c82040672daca5d56d7cece1f157836d01af5afda0fdfd7a064'),
            "selected/contracts/LOCAL-RECIPES.md": ("crates/toolkit-package/LOCAL-RECIPES.md", "d9856fa90d35cf8297401db21697fdd14ce186a22e96de213f508d5ba788f094"),
            "selected/contracts/INSPECT.md": ("crates/toolkit-package/INSPECT.md", "827b35792e4bc98f32afb59c7fb59b3091a137d0a4af7f052345d38fb788e7bf"),
            "selected/contracts/HOSTED-METADATA.md": ("sdk/consumer-setup/HOSTED-METADATA.md", "0465e739af8119489899f4958b4d53c177017e93e6357aa223565e671a7f3746"),
            "selected/contracts/SETUP-RECIPES.md": ("sdk/consumer-setup/RECIPES.md", "ce486049ec9166fe285c03297a63c24b1949fb555cb624acd2cf41519f1b359e"),
            "selected/contracts/HOSTED-RECIPES.md": ("sdk/consumer-setup/HOSTED-RECIPES.md", "42a8548a7cfbdf7868a98c64fa98ad8d24a6b4aa426fcf07cb1b2630a8bebda6"),
        },
    },
    "linux-x86_64": {
        "archive": "dist/agent-install-kit-20261004-node-linux/agent-install-kit-linux-x86_64.tar.gz",
        "sha256": "a5a75067b5e9c554dac3e2bde843152a8e9ceedd1c9b6bbc91c2eaa4be34cb46",
        "root": "agent-install-kit-linux-x86_64",
        'tool_updates': {'bin/toolkit-package-inspect': {'record': 'docs/verification/20261008-linux-mcpb/provenance/toolkit-package-inspect-build.json',
                                         'sha256': 'ceac75bd4dc054fa1eb2d3f9f361c8c46f4cd1ad79f1b844573ea8615ab071a4',
                                         'source': 'docs/verification/20261008-linux-mcpb/build-delivery/toolkit-package-inspect'}},
        'tool_additions': {'bin/toolkit-mcpb-install': {
                               'source': 'docs/verification/20261008-linux-mcpb/build-delivery/toolkit-mcpb-install',
                               'sha256': 'a39cac181ae79b237991203d265cb998f1245255851ba4d6f33fc52c35a90f25',
                               'record': 'docs/verification/20261008-linux-mcpb/provenance/toolkit-mcpb-install-build.json'},
                           'bin/toolkit-compose-plan': {'source': 'docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/delivery/toolkit-compose-plan',
                                       'sha256': 'a29f1a2249ff490f15f3aa35265b0a42727122d22aa14256bf961929c8c0d61a',
                                       'record': 'docs/verification/20261006-compose-migration-delivery/author/provenance/linux-release-binary.json'},
                           'bin/toolkit-local-recipes': {'record': 'docs/verification/20261006-portable-json-delivery/linux-recipes-release-binary.json',
                                       'sha256': 'b40fbad6a3f66b4eab7e588464d3e3954ee7f4ef0b41869393496a04d3d56a69',
                                       'source': 'dist/portable-json-20261006-linux-x86_64/toolkit-local-recipes'}},
        'capabilities': ['agent-plugin-files', 'mcpb-archive-inspection', 'compose-local-execution'],
        'sources': {
            'selected/provenance/compose-migration/source.json': ('docs/verification/20261006-compose-migration-delivery/author/provenance/source.json', '384b9b34a94f211aee2304704f600c0f198d99e6f94568446712e0865b77abb0'),
            'selected/provenance/compose-migration/source.tar.gz': ('docs/verification/20261006-compose-migration-delivery/author/provenance/source.tar.gz', '5763a04898122377ff8cc3746bb7f5a31b1bed120cfaa90c4368981af997c352'),
            'selected/provenance/compose-migration/linux-build.json': ('docs/verification/20261006-compose-migration-delivery/author/provenance/linux-build.json', '29bb0131f50354017e7ff2899aff5a43dab27fdade5ad42e190732da18c81ea7'),
            'selected/provenance/compose-migration/linux-dependencies.json': ('docs/verification/20261006-compose-migration-delivery/author/provenance/linux-dependencies.json', 'a5bd864a4425defcf541a52d8da039a2ed430c078c966e4c0bfa1a9e8dfd662b'),
            'selected/provenance/compose-migration/build-execution.json': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/execution/execution.json', '62146fe921062f29d1e12f29b46e5ca228db64077fd8b4bc35dff58ece8bd031'),
            'selected/provenance/compose-migration/build-preflight.json': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/execution/pre-entry.json', 'fb0c66910bff1a4726150c0a0a65b71e3d97276aa138cc1d245a4499dbb4d3b3'),
            'selected/provenance/compose-migration/build.stdout': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/execution/build.stdout', '481cfdd786c2fde7ba3e799e91ac20c26fb6a316f1a76bb321f8a0fb19e4f0fd'),
            'selected/provenance/compose-migration/build.stderr': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/execution/build.stderr', '0a97ccbee0ba56715644c91e06aeaddc2eab78ab4ee9bd3a47e82f4e8a63aa0a'),
            'selected/provenance/compose-migration/compiler-artifacts.json': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/execution/compiler-artifacts.json', 'bf35d89a43bf4d138fe35459d57f42ad4718de62835364b8c475ee99912929ff'),
            'selected/provenance/compose-migration/binary-metadata.txt': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/evidence/toolkit-compose-plan.metadata.txt', '48f9f904eb141692bcb9ae023a14767d89bfde3f7460f54a6f8f7a35a88fdd08'),
            'selected/provenance/compose-migration/rust-toolchain-notices.sha256': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/evidence/rust-toolchain-notices.sha256', 'cbee08ad481b3ed511cc86162fd336c4c56ea9657f621819b1fe9ecf5ee6178f'),
            'selected/provenance/compose-migration/post-verify.json': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/verify-post-build.json', '10b025c95b52b088cc9b531733ad30004e9c1626985668b872157ec44cacbf5e'),
            'selected/provenance/compose-migration/target-cleanup.txt': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/returned/target-reclaimed-bytes.txt', 'b701fa73117e27c1795916fb66e3a363a0bfd9350345eece5f62a53d5194bba7'),
            'selected/provenance/compose-migration/build-readback.json': ('docs/verification/20261006-compose-migration-delivery/linux-build-receipt/readback.json', 'eae08c57fdf1f8607f11e40922026469603df8902de5fd27ea26c15851924f05'),
            'selected/provenance/compose-migration/launch_build.py': ('docs/verification/20261006-compose-migration-delivery/author/linux-prep/launch_build.py', 'fc7b008922fcd85d986c4fe61062f449e7827c6a6d9242c073df4d2a3f9956d8'),
            'selected/provenance/compose-migration/dispatch-inputs.json': ('docs/verification/20261006-compose-migration-delivery/author/linux-prep/dispatch-inputs.json', 'ed57143175b4f9f46afc975446310adc4d1a2d0920a97f6ac410db707d69dda2'),
            'selected/provenance/compose-retention/source.json': ('docs/verification/20261005-compose-retention-delivery/author/provenance/source.json', '39f4e89091e9b2cd83959880149c5f99b9e47726a08315ed86a6158fd41e31c2'),
            'selected/provenance/compose-retention/source.tar.gz': ('docs/verification/20261005-compose-retention-delivery/author/provenance/source.tar.gz', '8a56cbf81dfc86adf10579212e79b755f7f490a8fae35d6047fc151d6fee54cd'),
            'selected/provenance/compose-retention/linux-build.json': ('docs/verification/20261005-compose-retention-delivery/author/provenance/linux-build.json', '359037d31f3dee19d7ce459f60644b9d236402d6d9769f494633fa52950ba2da'),
            'selected/provenance/compose-retention/linux-dependencies.json': ('docs/verification/20261005-compose-retention-delivery/author/provenance/linux-dependencies.json', '5c79983a4be3a9a24d16b2c1662583b4c52a9cb99bb1e7306c9c8030a58cb2b3'),
            'selected/provenance/compose-retention/build-execution.json': ('docs/verification/20261005-compose-retention-delivery/linux-build-receipt/remote/execution/execution.json', '3cd408356293c791b107f951e41808078173b0e7ece87340713261358e55d3fc'),
            'selected/provenance/compose-retention/build-preflight.json': ('docs/verification/20261005-compose-retention-delivery/linux-build-receipt/remote/execution/pre-entry.json', '880453ff3fd6479eeccfa7d3b4831c3efb2f03a949ebde0b1a7cdef86d7161f5'),
            'selected/provenance/compose-retention/build.stdout': ('docs/verification/20261005-compose-retention-delivery/linux-build-receipt/remote/execution/build.stdout', '916f09a9de50886e5666781dc26371c920c36be7a75eea7e873834c1a453a953'),
            'selected/provenance/compose-retention/build.stderr': ('docs/verification/20261005-compose-retention-delivery/linux-build-receipt/remote/execution/build.stderr', '84b34191ad96ab73780f1323077c58f6a920b4fadc60bfc3fe0147f3e4640b2f'),
            'selected/provenance/compose-retention/release-artifacts.json': ('docs/verification/20261005-compose-retention-delivery/linux-build-receipt/remote/execution/release-artifacts.json', 'f21b813cabf2fb4d72610b741e3b009f4baca654b86a20f60960f5e4ee0e0a4d'),
            'selected/provenance/compose-retention/binary-record.txt': ('docs/verification/20261005-compose-retention-delivery/linux-build-receipt/remote/execution/binary-record.txt', '36344b86389080d94c2123e392a7a3793f4464283c87825499abc165e7a17b15'),
            'selected/provenance/compose-retention/post-verify.json': ('docs/verification/20261005-compose-retention-delivery/linux-build-receipt/remote/execution/post-verify.json', '10b025c95b52b088cc9b531733ad30004e9c1626985668b872157ec44cacbf5e'),
            'selected/provenance/compose-retention/launch_build.py': ('docs/verification/20261005-compose-retention-delivery/author/linux-prep/launch_build.py', 'fd24fa694d84b6ce4d84b93fe2ecb9c77673d91012c44879c3037d63083342ac'),
            'selected/provenance/compose-retention/dispatch-inputs.json': ('docs/verification/20261005-compose-retention-delivery/author/linux-prep/dispatch-inputs.json', 'a76e6132a29127b79e7a2b3f863d94bc531a010f5ef2eb1dd3bf9a44cdd64fe5'),
            'selected/licenses/crates/serde_path_to_error-0.1.20/LICENSE-APACHE': ('docs/verification/20261005-package-diagnostics/notices/licenses/crates/serde_path_to_error-0.1.20/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
            'selected/licenses/crates/serde_path_to_error-0.1.20/LICENSE-MIT': ('docs/verification/20261005-package-diagnostics/notices/licenses/crates/serde_path_to_error-0.1.20/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
            'selected/provenance/crates/serde_path_to_error-0.1.20.crate': ('docs/verification/20261005-package-diagnostics/notices/provenance/crates/serde_path_to_error-0.1.20.crate', '10a9ff822e371bb5403e391ecd83e182e0e77ba7f6fe0160b795797109d1b457'),
            'selected/provenance/package-diagnostics-linux/dependency-coverage.json': ('docs/verification/20261005-package-diagnostics/linux-run/notices/coverage.json', 'a7899477be48838993e7067b9c5afc10a8fe8f13d114bf38175ae8328dac56ed'),
            'selected/provenance/package-diagnostics-linux/serde_path_to_error-0.1.20.source.json': ('docs/verification/20261005-package-diagnostics/linux-run/notices/serde_path_to_error-0.1.20.source.json', '4b7a896233df99a397e55738bae9c212eb2cae220485b2c735df9b9f7ddc3dd4'),
            'selected/contracts/HOSTED-METADATA.md': ('sdk/consumer-setup/HOSTED-METADATA.md',
                                                   '0465e739af8119489899f4958b4d53c177017e93e6357aa223565e671a7f3746'),
         'selected/contracts/HOSTED-RECIPES.md': ('sdk/consumer-setup/HOSTED-RECIPES.md',
                                                  '42a8548a7cfbdf7868a98c64fa98ad8d24a6b4aa426fcf07cb1b2630a8bebda6'),
         'selected/contracts/INSPECT.md': ('crates/toolkit-package/INSPECT.md',
                                           '827b35792e4bc98f32afb59c7fb59b3091a137d0a4af7f052345d38fb788e7bf'),
         'selected/contracts/LOCAL-RECIPES.md': ('crates/toolkit-package/LOCAL-RECIPES.md',
                                                 'd9856fa90d35cf8297401db21697fdd14ce186a22e96de213f508d5ba788f094'),
         'selected/contracts/SETUP-RECIPES.md': ('sdk/consumer-setup/RECIPES.md',
                                                 'ce486049ec9166fe285c03297a63c24b1949fb555cb624acd2cf41519f1b359e'),
         'selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/COPYRIGHT',
                                                                                              'd00d6d48df2db6f91e4ca9c656e5e30782d5becf52bb3e5bfd0aff467bd37781'),
         'selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/LICENSE-APACHE',
                                                                                                   'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                           '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/ambient-authority-0.0.2/LICENSE-MIT',
                                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/async-trait-0.1.92/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/async-trait-0.1.92/LICENSE-APACHE',
                                                                                              '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/async-trait-0.1.92/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/async-trait-0.1.92/LICENSE-MIT',
                                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/bitflags-2.13.2/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/bitflags-2.13.2/LICENSE-APACHE',
                                                                                           'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/bitflags-2.13.2/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/bitflags-2.13.2/LICENSE-MIT',
                                                                                        '6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb'),
         'selected/licenses/linux-plugin-guidance/crates/block-buffer-0.10.4/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/block-buffer-0.10.4/LICENSE-APACHE',
                                                                                               'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
         'selected/licenses/linux-plugin-guidance/crates/block-buffer-0.10.4/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/block-buffer-0.10.4/LICENSE-MIT',
                                                                                            'd5c22aa3118d240e877ad41c5d9fa232f9c77d757d4aac0c2f943afc0a95e0ef'),
         'selected/licenses/linux-plugin-guidance/crates/bytes-1.12.1/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/bytes-1.12.1/LICENSE',
                                                                                 '45f522cacecb1023856e46df79ca625dfc550c94910078bd8aec6e02880b3d42'),
         'selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/COPYRIGHT',
                                                                                           'a8799c2a07dabcb8d52481f885c5ab7000acb8a4d3cd9503fbcb97c84d350e65'),
         'selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/LICENSE-APACHE',
                                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                        '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-primitives-4.0.3/LICENSE-MIT',
                                                                                             '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/COPYRIGHT',
                                                                                    '6b1691fdee3b03c3946696a7be125fcf50eff310b9486fac850c1b8aea91478b'),
         'selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/LICENSE-APACHE',
                                                                                         'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                 '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cap-std-4.0.3/LICENSE-MIT',
                                                                                      '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/cc-1.5.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cc-1.5.1/LICENSE-APACHE',
                                                                                    'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/cc-1.5.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cc-1.5.1/LICENSE-MIT',
                                                                                 '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
         'selected/licenses/linux-plugin-guidance/crates/cfg-if-1.0.5/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cfg-if-1.0.5/LICENSE-APACHE',
                                                                                        'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/cfg-if-1.0.5/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cfg-if-1.0.5/LICENSE-MIT',
                                                                                     '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
         'selected/licenses/linux-plugin-guidance/crates/cpufeatures-0.2.17/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cpufeatures-0.2.17/LICENSE-APACHE',
                                                                                              'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
         'selected/licenses/linux-plugin-guidance/crates/cpufeatures-0.2.17/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/cpufeatures-0.2.17/LICENSE-MIT',
                                                                                           'ae9baa7beea910273c2f384c2a6b721fb7bd02bda3436074a1072e4ee689f985'),
         'selected/licenses/linux-plugin-guidance/crates/crypto-common-0.1.7/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/crypto-common-0.1.7/LICENSE-APACHE',
                                                                                               'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
         'selected/licenses/linux-plugin-guidance/crates/crypto-common-0.1.7/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/crypto-common-0.1.7/LICENSE-MIT',
                                                                                            '3521672491a3479422d5fe1aca6645dd2984090f85da6e5205abfb18fb7a6897'),
         'selected/licenses/linux-plugin-guidance/crates/digest-0.10.7/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/digest-0.10.7/LICENSE-APACHE',
                                                                                         'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
         'selected/licenses/linux-plugin-guidance/crates/digest-0.10.7/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/digest-0.10.7/LICENSE-MIT',
                                                                                      '9e0dfd2dd4173a530e238cb6adb37aa78c34c6bc7444e0e10c1ab5d8881f63ba'),
         'selected/licenses/linux-plugin-guidance/crates/displaydoc-0.2.7/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/displaydoc-0.2.7/LICENSE-APACHE',
                                                                                            'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/displaydoc-0.2.7/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/displaydoc-0.2.7/LICENSE-MIT',
                                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/errno-0.3.14/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/errno-0.3.14/LICENSE-APACHE',
                                                                                        'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/errno-0.3.14/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/errno-0.3.14/LICENSE-MIT',
                                                                                     '8764a597675778ddfd4e25f81b08a05dbcf089ac05662df7613fe67f150e3aa2'),
         'selected/licenses/linux-plugin-guidance/crates/fallible-iterator-0.3.0/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fallible-iterator-0.3.0/LICENSE-APACHE',
                                                                                                   'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
         'selected/licenses/linux-plugin-guidance/crates/fallible-iterator-0.3.0/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fallible-iterator-0.3.0/LICENSE-MIT',
                                                                                                '0816e154b159ba255c563f7c8c7df5bbb8cc5fc96f5ab8cf9f4743b4f41fe7eb'),
         'selected/licenses/linux-plugin-guidance/crates/fallible-streaming-iterator-0.1.9/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fallible-streaming-iterator-0.1.9/LICENSE-APACHE',
                                                                                                             'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
         'selected/licenses/linux-plugin-guidance/crates/fallible-streaming-iterator-0.1.9/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fallible-streaming-iterator-0.1.9/LICENSE-MIT',
                                                                                                          '8dcec5569a9be5b0e086c80faed6f1aefa670af0ec29cecc2f714303096887e0'),
         'selected/licenses/linux-plugin-guidance/crates/fastrand-2.5.0/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fastrand-2.5.0/LICENSE-APACHE',
                                                                                          'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/fastrand-2.5.0/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fastrand-2.5.0/LICENSE-MIT',
                                                                                       '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/find-msvc-tools-0.1.14/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/find-msvc-tools-0.1.14/LICENSE-APACHE',
                                                                                                  'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/find-msvc-tools-0.1.14/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/find-msvc-tools-0.1.14/LICENSE-MIT',
                                                                                               '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
         'selected/licenses/linux-plugin-guidance/crates/foldhash-0.1.5/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/foldhash-0.1.5/LICENSE',
                                                                                   'b1181a40b2a7b25cf66fd01481713bc1005df082c53ef73e851e55071b102744'),
         'selected/licenses/linux-plugin-guidance/crates/form_urlencoded-1.2.2/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/form_urlencoded-1.2.2/LICENSE-APACHE',
                                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/form_urlencoded-1.2.2/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/form_urlencoded-1.2.2/LICENSE-MIT',
                                                                                              '20c7855c364d57ea4c97889a5e8d98470a9952dade37bd9248b9a54431670e5e'),
         'selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/COPYRIGHT',
                                                                                          'e196ffeb5ab101e9ec8bbe395c9f367c5380ee2d28c47e5c03371523798a9af7'),
         'selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/LICENSE-APACHE',
                                                                                               'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                       '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/fs-set-times-0.20.3/LICENSE-MIT',
                                                                                            '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/generic-array-0.14.7/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/generic-array-0.14.7/LICENSE',
                                                                                         'c09aae9d3c77b531f56351a9947bc7446511d6b025b3255312d3e3442a9a7583'),
         'selected/licenses/linux-plugin-guidance/crates/getrandom-0.4.3/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/getrandom-0.4.3/LICENSE-APACHE',
                                                                                           'aaff376532ea30a0cd5330b9502ad4a4c8bf769c539c87ffe78819d188a18ebf'),
         'selected/licenses/linux-plugin-guidance/crates/getrandom-0.4.3/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/getrandom-0.4.3/LICENSE-MIT',
                                                                                        '523a42c25d245dde9c015f882cec7f4555aad883382a6cf19b4b7d9b2cd5419b'),
         'selected/licenses/linux-plugin-guidance/crates/hashbrown-0.15.5/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/hashbrown-0.15.5/LICENSE-APACHE',
                                                                                            'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/hashbrown-0.15.5/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/hashbrown-0.15.5/LICENSE-MIT',
                                                                                         'ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2'),
         'selected/licenses/linux-plugin-guidance/crates/hashlink-0.10.0/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/hashlink-0.10.0/LICENSE-APACHE',
                                                                                           'c144680885b29e4719e2a51f0aab5439a1e02d980692b5aaf086cae12727f28b'),
         'selected/licenses/linux-plugin-guidance/crates/hashlink-0.10.0/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/hashlink-0.10.0/LICENSE-MIT',
                                                                                        'e915669a595b11a200873df8286561881b0e04932f6412a585db6297ba0bc97c'),
         'selected/licenses/linux-plugin-guidance/crates/hex-0.4.3/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/hex-0.4.3/LICENSE-APACHE',
                                                                                     'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
         'selected/licenses/linux-plugin-guidance/crates/hex-0.4.3/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/hex-0.4.3/LICENSE-MIT',
                                                                                  'f7bdb3426d045cd50efd4953026e3eb5a83d0199f458a075602611b9344da5b9'),
         'selected/licenses/linux-plugin-guidance/crates/icu_collections-2.3.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/icu_collections-2.3.0/LICENSE',
                                                                                          'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/icu_locale_core-2.3.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/icu_locale_core-2.3.0/LICENSE',
                                                                                          'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/icu_normalizer-2.3.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/icu_normalizer-2.3.0/LICENSE',
                                                                                         'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/icu_normalizer_data-2.3.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/icu_normalizer_data-2.3.0/LICENSE',
                                                                                              'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/icu_properties-2.3.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/icu_properties-2.3.0/LICENSE',
                                                                                         'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/icu_properties_data-2.3.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/icu_properties_data-2.3.0/LICENSE',
                                                                                              'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/icu_provider-2.3.1/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/icu_provider-2.3.1/LICENSE',
                                                                                       'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/idna-1.1.0/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/idna-1.1.0/LICENSE-APACHE',
                                                                                      'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/idna-1.1.0/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/idna-1.1.0/LICENSE-MIT',
                                                                                   'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
         'selected/licenses/linux-plugin-guidance/crates/idna_adapter-1.2.2/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/idna_adapter-1.2.2/LICENSE-APACHE',
                                                                                              'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/idna_adapter-1.2.2/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/idna_adapter-1.2.2/LICENSE-MIT',
                                                                                           '8b43ce8accd61e9d370b5ca9e9c4f953279b5c239926c62315b40e24df51b726'),
         'selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/COPYRIGHT',
                                                                                       'f0c1d22de8bd3b776a40f2691f08b84e91c7bafc78b72a51150e231002b88db3'),
         'selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/LICENSE-APACHE',
                                                                                            'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                    '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-extras-0.19.0/LICENSE-MIT',
                                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/COPYRIGHT',
                                                                                         '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/LICENSE-APACHE',
                                                                                              'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                      '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-2.0.4/LICENSE-MIT',
                                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/COPYRIGHT',
                                                                                         '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/LICENSE-APACHE',
                                                                                              'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                      '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/io-lifetimes-3.0.1/LICENSE-MIT',
                                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/ipnet-2.12.2/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/ipnet-2.12.2/LICENSE-APACHE',
                                                                                        '87d9feb9238c6bd8e0024fc4733b06cff036f89f36d93b7df1c8a0549bbb7a5b'),
         'selected/licenses/linux-plugin-guidance/crates/ipnet-2.12.2/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/ipnet-2.12.2/LICENSE-MIT',
                                                                                     '47dc9ff29128ddfb4d6a0435383c9f89120bc374dbcc1dd00b933a0b28aa7865'),
         'selected/licenses/linux-plugin-guidance/crates/itoa-1.0.18/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/itoa-1.0.18/LICENSE-APACHE',
                                                                                       '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/itoa-1.0.18/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/itoa-1.0.18/LICENSE-MIT',
                                                                                    '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/libc-0.2.189/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/libc-0.2.189/LICENSE-APACHE',
                                                                                        '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/libc-0.2.189/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/libc-0.2.189/LICENSE-MIT',
                                                                                     '123a331b5dbf04c30097fa43b8f858bc85df671fe776de498d01f3d6b7c1f69e'),
         'selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/LICENSE',
                                                                                          'c10c1f27337546471e5f7e4e97fdd398b35b9d4e126115dcd22de8d8e65abf6f'),
         'selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/sqlcipher/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/sqlcipher/LICENSE',
                                                                                                    'ea4fcb309f14a22065e1ea45362d494d320012249ed865fe9c7c0946db754131'),
         'selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/sqlite3/sqlite3.c.attribution.txt': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/sqlite3/sqlite3.c.attribution.txt',
                                                                                                                    '39fc4986b224f2f5575dec58ae66c7c9ee2d5d3884ba4971c932f1fd5b40b033'),
         'selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/sqlite3/sqlite3.h.attribution.txt': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/libsqlite3-sys-0.35.0/sqlite3/sqlite3.h.attribution.txt',
                                                                                                                    'cc65d76f325d884dd49e319acf3764d5513920f34ad18e34347b8dedc6ddd2ec'),
         'selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/COPYRIGHT',
                                                                                           '3290ae0fbc9ddb77d2239121d710f0bb9d31b3b4744e6d97fe01e652b4c1870b'),
         'selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/LICENSE-APACHE',
                                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                        '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/linux-raw-sys-0.12.1/LICENSE-MIT',
                                                                                             '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/litemap-0.8.3/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/litemap-0.8.3/LICENSE',
                                                                                  'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/maybe-owned-0.3.4/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/maybe-owned-0.3.4/LICENSE-APACHE',
                                                                                             'aecc100d0547aa3bcbe04bcf5cb87c7db0be11004602adfa5056edf8bb847b35'),
         'selected/licenses/linux-plugin-guidance/crates/maybe-owned-0.3.4/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/maybe-owned-0.3.4/LICENSE-MIT',
                                                                                          'ef2b3bbd7b718a7881253226313f103e89b379e1279681a3c1ebcb65a27d209d'),
         'selected/licenses/linux-plugin-guidance/crates/memchr-2.8.3/COPYING': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/memchr-2.8.3/COPYING',
                                                                                 '01c266bced4a434da0051174d6bee16a4c82cf634e2679b6155d40d75012390f'),
         'selected/licenses/linux-plugin-guidance/crates/memchr-2.8.3/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/memchr-2.8.3/LICENSE-MIT',
                                                                                     '0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f'),
         'selected/licenses/linux-plugin-guidance/crates/memchr-2.8.3/UNLICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/memchr-2.8.3/UNLICENSE',
                                                                                   '7e12e5df4bae12cb21581ba157ced20e1986a0508dd10d0e8a4ab9a4cf94e85c'),
         'selected/licenses/linux-plugin-guidance/crates/mio-1.2.3/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/mio-1.2.3/LICENSE',
                                                                              '07919255c7e04793d8ea760d6c2ce32d19f9ff02bdbdde3ce90b1e1880929a9b'),
         'selected/licenses/linux-plugin-guidance/crates/once_cell-1.21.4/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/once_cell-1.21.4/LICENSE-APACHE',
                                                                                            'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/once_cell-1.21.4/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/once_cell-1.21.4/LICENSE-MIT',
                                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/percent-encoding-2.3.2/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/percent-encoding-2.3.2/LICENSE-APACHE',
                                                                                                  'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/percent-encoding-2.3.2/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/percent-encoding-2.3.2/LICENSE-MIT',
                                                                                               'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
         'selected/licenses/linux-plugin-guidance/crates/pin-project-lite-0.2.17/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/pin-project-lite-0.2.17/LICENSE-APACHE',
                                                                                                   '0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594'),
         'selected/licenses/linux-plugin-guidance/crates/pin-project-lite-0.2.17/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/pin-project-lite-0.2.17/LICENSE-MIT',
                                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/pkg-config-0.3.34/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/pkg-config-0.3.34/LICENSE-APACHE',
                                                                                             'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/pkg-config-0.3.34/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/pkg-config-0.3.34/LICENSE-MIT',
                                                                                          '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
         'selected/licenses/linux-plugin-guidance/crates/potential_utf-0.1.6/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/potential_utf-0.1.6/LICENSE',
                                                                                        'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/proc-macro2-1.0.107/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/proc-macro2-1.0.107/LICENSE-APACHE',
                                                                                               '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/proc-macro2-1.0.107/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/proc-macro2-1.0.107/LICENSE-MIT',
                                                                                            '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/quote-1.0.47/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/quote-1.0.47/LICENSE-APACHE',
                                                                                        '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/quote-1.0.47/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/quote-1.0.47/LICENSE-MIT',
                                                                                     '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/rusqlite-0.37.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rusqlite-0.37.0/LICENSE',
                                                                                    'c10c1f27337546471e5f7e4e97fdd398b35b9d4e126115dcd22de8d8e65abf6f'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/COPYRIGHT',
                                                                                   '377c2e7c53250cc5905c0b0532d35973392af16ffb9596a41d99d202cf3617c9'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/LICENSE-APACHE',
                                                                                        'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-1.1.5/LICENSE-MIT',
                                                                                     '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/COPYRIGHT',
                                                                                                'd415a86ccfd794129666247cbbadebee7a6d0ac3ff701c76c07b0f9424f0307d'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/LICENSE-APACHE',
                                                                                                     'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                             '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
         'selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/rustix-linux-procfs-0.1.1/LICENSE-MIT',
                                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/serde-1.0.229/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde-1.0.229/LICENSE-APACHE',
                                                                                         '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/serde-1.0.229/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde-1.0.229/LICENSE-MIT',
                                                                                      '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/serde_core-1.0.229/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde_core-1.0.229/LICENSE-APACHE',
                                                                                              '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/serde_core-1.0.229/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde_core-1.0.229/LICENSE-MIT',
                                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/serde_derive-1.0.229/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde_derive-1.0.229/LICENSE-APACHE',
                                                                                                '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/serde_derive-1.0.229/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde_derive-1.0.229/LICENSE-MIT',
                                                                                             '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/serde_json-1.0.151/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde_json-1.0.151/LICENSE-APACHE',
                                                                                              '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/serde_json-1.0.151/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/serde_json-1.0.151/LICENSE-MIT',
                                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/sha2-0.10.9/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/sha2-0.10.9/LICENSE-APACHE',
                                                                                       'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
         'selected/licenses/linux-plugin-guidance/crates/sha2-0.10.9/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/sha2-0.10.9/LICENSE-MIT',
                                                                                    'b4eb00df6e2a4d22518fcaa6a2b4646f249b3a3c9814509b22bd2091f1392ff1'),
         'selected/licenses/linux-plugin-guidance/crates/shlex-2.0.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/shlex-2.0.1/LICENSE-APACHE',
                                                                                       '553fffcd9b1cb158bc3e9edc35da85ca5c3b3d7d2e61c883ebcfa8a65814b583'),
         'selected/licenses/linux-plugin-guidance/crates/shlex-2.0.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/shlex-2.0.1/LICENSE-MIT',
                                                                                    '4455bf75a91154108304cb283e0fea9948c14f13e20d60887cf2552449dea3b1'),
         'selected/licenses/linux-plugin-guidance/crates/signal-hook-registry-1.4.8/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/signal-hook-registry-1.4.8/LICENSE-APACHE',
                                                                                                      'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/signal-hook-registry-1.4.8/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/signal-hook-registry-1.4.8/LICENSE-MIT',
                                                                                                   '503558bfefe66ca15e4e3f7955b3cb0ec87fd52f29bf24b336af7bd00e946d5c'),
         'selected/licenses/linux-plugin-guidance/crates/smallvec-1.16.2/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/smallvec-1.16.2/LICENSE-APACHE',
                                                                                           'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/smallvec-1.16.2/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/smallvec-1.16.2/LICENSE-MIT',
                                                                                        '0b28172679e0009b655da42797c03fd163a3379d5cfa67ba1f1655e974a2a1a9'),
         'selected/licenses/linux-plugin-guidance/crates/socket2-0.6.5/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/socket2-0.6.5/LICENSE-APACHE',
                                                                                         'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/socket2-0.6.5/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/socket2-0.6.5/LICENSE-MIT',
                                                                                      '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
         'selected/licenses/linux-plugin-guidance/crates/stable_deref_trait-1.2.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/stable_deref_trait-1.2.1/LICENSE-APACHE',
                                                                                                    'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/stable_deref_trait-1.2.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/stable_deref_trait-1.2.1/LICENSE-MIT',
                                                                                                 '5e05b024f653a5ce199e77cbbbd42fb5553562ec714b819421ed0c3e552a75d7'),
         'selected/licenses/linux-plugin-guidance/crates/syn-3.0.6/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/syn-3.0.6/LICENSE-APACHE',
                                                                                     '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/syn-3.0.6/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/syn-3.0.6/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/synstructure-0.14.0/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/synstructure-0.14.0/LICENSE',
                                                                                        '219920e865eee70b7dcfc948a86b099e7f4fe2de01bcca2ca9a20c0a033f2b59'),
         'selected/licenses/linux-plugin-guidance/crates/tempfile-3.27.0/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/tempfile-3.27.0/LICENSE-APACHE',
                                                                                           'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/tempfile-3.27.0/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/tempfile-3.27.0/LICENSE-MIT',
                                                                                        '8b427f5bc501764575e52ba4f9d95673cf8f6d80a86d0d06599852e1a9a20a36'),
         'selected/licenses/linux-plugin-guidance/crates/thiserror-2.0.21/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/thiserror-2.0.21/LICENSE-APACHE',
                                                                                            '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/thiserror-2.0.21/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/thiserror-2.0.21/LICENSE-MIT',
                                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/thiserror-impl-2.0.21/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/thiserror-impl-2.0.21/LICENSE-APACHE',
                                                                                                 '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/thiserror-impl-2.0.21/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/thiserror-impl-2.0.21/LICENSE-MIT',
                                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/tinystr-0.8.4/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/tinystr-0.8.4/LICENSE',
                                                                                  'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/tokio-1.53.1/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/tokio-1.53.1/LICENSE',
                                                                                 '253cd04c6714889df2d32f3f64d669179a1c95c76ac43c40882c52eb06bc3552'),
         'selected/licenses/linux-plugin-guidance/crates/tokio-macros-2.7.2/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/tokio-macros-2.7.2/LICENSE',
                                                                                       '0b83dc40cba89b9922bb84b0a9c7d2768ce37c1d7e138b7424fd4549915778c9'),
         'selected/licenses/linux-plugin-guidance/crates/typenum-1.20.1/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/typenum-1.20.1/LICENSE',
                                                                                   'db11fec9946737df39ca3898d9cd8c10ec6f6c3a884a6802b0ad0b81b4e8f23a'),
         'selected/licenses/linux-plugin-guidance/crates/typenum-1.20.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/typenum-1.20.1/LICENSE-APACHE',
                                                                                          '516b24e051bf5630880ebbd55c40a25ce9552ebaf8970a53e8976eb70e522406'),
         'selected/licenses/linux-plugin-guidance/crates/typenum-1.20.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/typenum-1.20.1/LICENSE-MIT',
                                                                                       'a825bd853ab71619a4923d7b4311221427848070ff44d990da39b0b274c1683f'),
         'selected/licenses/linux-plugin-guidance/crates/unicode-ident-1.0.26/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/unicode-ident-1.0.26/LICENSE-APACHE',
                                                                                                '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
         'selected/licenses/linux-plugin-guidance/crates/unicode-ident-1.0.26/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/unicode-ident-1.0.26/LICENSE-MIT',
                                                                                             '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/crates/unicode-ident-1.0.26/LICENSE-UNICODE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/unicode-ident-1.0.26/LICENSE-UNICODE',
                                                                                                 'f7db81051789b729fea528a63ec4c938fdcb93d9d61d97dc8cc2e9df6d47f2a1'),
         'selected/licenses/linux-plugin-guidance/crates/url-2.5.8/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/url-2.5.8/LICENSE-APACHE',
                                                                                     'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/url-2.5.8/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/url-2.5.8/LICENSE-MIT',
                                                                                  'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
         'selected/licenses/linux-plugin-guidance/crates/utf8_iter-1.0.4/COPYRIGHT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/utf8_iter-1.0.4/COPYRIGHT',
                                                                                      'c30152c94a6d75e021adbc52b3a52470366a46edb917e17deae3259251af244c'),
         'selected/licenses/linux-plugin-guidance/crates/utf8_iter-1.0.4/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/utf8_iter-1.0.4/LICENSE-APACHE',
                                                                                           'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'),
         'selected/licenses/linux-plugin-guidance/crates/utf8_iter-1.0.4/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/utf8_iter-1.0.4/LICENSE-MIT',
                                                                                        '3fa4ca83dcc9237839b1bdeb2e6d16bdfb5ec0c5ce42b24694d8bbf0dcbef72c'),
         'selected/licenses/linux-plugin-guidance/crates/uuid-1.26.1/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/uuid-1.26.1/LICENSE-APACHE',
                                                                                       'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/uuid-1.26.1/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/uuid-1.26.1/LICENSE-MIT',
                                                                                    '436bc5a105d8e57dcd8778730f3754f7bf39c14d2f530e4cde4bd2d17a83ec3d'),
         'selected/licenses/linux-plugin-guidance/crates/vcpkg-0.2.15/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/vcpkg-0.2.15/LICENSE-APACHE',
                                                                                        '60c93a31f490375aadf64098b75f10715010379541021e00261045d7800611d3'),
         'selected/licenses/linux-plugin-guidance/crates/vcpkg-0.2.15/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/vcpkg-0.2.15/LICENSE-MIT',
                                                                                     '016d20f335060a70e79d9fcf8dfaa6201114d65d592211f4bdfb8ae9ca2bc1dc'),
         'selected/licenses/linux-plugin-guidance/crates/version_check-0.9.5/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/version_check-0.9.5/LICENSE-APACHE',
                                                                                               'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
         'selected/licenses/linux-plugin-guidance/crates/version_check-0.9.5/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/version_check-0.9.5/LICENSE-MIT',
                                                                                            'b7e650f3fce5c53249d1cdc608b54df156a97edd636cf9d23498d0cfe7aec63e'),
         'selected/licenses/linux-plugin-guidance/crates/writeable-0.6.4/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/writeable-0.6.4/LICENSE',
                                                                                    'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/yoke-0.8.3/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/yoke-0.8.3/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/yoke-derive-0.8.3/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/yoke-derive-0.8.3/LICENSE',
                                                                                      'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/zerofrom-0.1.8/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/zerofrom-0.1.8/LICENSE',
                                                                                   'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/zerofrom-derive-0.1.8/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/zerofrom-derive-0.1.8/LICENSE',
                                                                                          'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/zerotrie-0.2.5/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/zerotrie-0.2.5/LICENSE',
                                                                                   'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/zerovec-0.11.8/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/zerovec-0.11.8/LICENSE',
                                                                                   'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/zerovec-derive-0.11.6/LICENSE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/zerovec-derive-0.11.6/LICENSE',
                                                                                          'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
         'selected/licenses/linux-plugin-guidance/crates/zmij-1.0.23/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/crates/zmij-1.0.23/LICENSE-MIT',
                                                                                    '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
         'selected/licenses/linux-plugin-guidance/rust/COPYRIGHT-library.html': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/rust/COPYRIGHT-library.html',
                                                                                 '68129500b616d5838629e68f55ff3aed5e096dacf60ce9eb41bbe599a563afa6'),
         'selected/licenses/linux-plugin-guidance/toolkit/LICENSE-APACHE': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/toolkit/LICENSE-APACHE',
                                                                            '3a0ffd02ca79c2a687921b0f5d8094f2f6a7058a4c01775d1351ea6514fcd180'),
         'selected/licenses/linux-plugin-guidance/toolkit/LICENSE-MIT': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/licenses/linux-plugin-guidance/toolkit/LICENSE-MIT',
                                                                         'b45d1f632f83aeb359da8e2b8716ad72591ec7267a489fdda39989c618d65a0f'),
         'selected/provenance/linux-plugin-guidance/Cargo.lock': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/provenance/linux-plugin-guidance/Cargo.lock',
                                                                  '4431d38ccd445da729dd49b52ff7b811bde4c6b2e1876efa2be2ff9d22fa9477'),
         'selected/provenance/linux-plugin-guidance/build-plan.json': ('docs/verification/20261004-linux-plugin-guidance/retry2/plan.json',
                                                                       'cbaec72b177e737cfbff2e4ab3fd905eb3b5d95d4304e7c9e1b4afbd85e1da67'),
         'selected/provenance/linux-plugin-guidance/build.json': ('docs/verification/20261004-linux-plugin-guidance/retry2/execution/result.json',
                                                                  '03c29b4ff89be20b99e4612cef569a864a1eabd2f3b26ee5d4053c4ffa98bfdc'),
         'selected/provenance/linux-plugin-guidance/consumer-pins.json': ('docs/verification/20261004-linux-plugin-guidance/retry2/consumer2/execution/pins.json',
                                                                          '65e7e9bddd3e94fb7d32800ea1da4788bd64944ccb273988fe41313977144860'),
         'selected/provenance/linux-plugin-guidance/consumer.json': ('docs/verification/20261004-linux-plugin-guidance/retry2/consumer2/execution/result.json',
                                                                     '063ac10a16d90e83c3def65e7743c9092d6deee117c6ece5f5d1bbf8832b88fe'),
         'selected/provenance/linux-plugin-guidance/dependencies.json': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/provenance/linux-plugin-guidance/dependencies.json',
                                                                         '5e68cd534ffbdc4243e340eb8916e57d5cfe6e91079b847f111231401b4f34f9'),
         'selected/provenance/linux-plugin-guidance/metadata-command.json': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/provenance/linux-plugin-guidance/metadata-command.json',
                                                                             '7fbf5cd8a39723f9818aa0c93759a685e7b6086b2070dccddcb816c778b36eff'),
         'selected/provenance/linux-plugin-guidance/metadata.json': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/provenance/linux-plugin-guidance/metadata.json',
                                                                     '93ebf4ea604e45eb2b56a2aef09c6c53e47503de8fcaec2d5ae54b89ad9177e8'),
         'selected/provenance/linux-plugin-guidance/rustc-version.txt': ('docs/verification/20261004-linux-plugin-guidance/notices/collected/files/selected/provenance/linux-plugin-guidance/rustc-version.txt',
                                                                         '778fbc643a806f0dd123c056e66201d7e8a56a61928ec0a33171be8d6c2d5a37'),
         'selected/provenance/linux-plugin-guidance/source-manifest.json': ('docs/verification/20261004-linux-plugin-guidance/retry2/source-manifest.json',
                                                                            '32b20e89d3ea32377ec66d508df7b30fcf7bc9515847123edab1e8a9e033a89f'),
         'selected/provenance/linux-plugin-guidance/source.patch': ('docs/verification/20261004-linux-plugin-guidance/retry2/source.patch',
                                                                    '6759bcf75c8e18f9c318bbf6abdd007b4150f0c4a8b1b35a0f8f758dadc169ea')},
        "tools": {
            "bin/install-library-consumer.py": "4b4c468d97f73d6ce60117ad6a48deb0ee5f291953fd7143744124f1819b89a0",
            "bin/toolkit-node-plan": "f17e921915faf266af7a95943049e2c66a50ce8b85b085d678d593d829300e45",
            "bin/toolkit-python-plan": "de4895a3f231e52c71256b412fbd265c03425ceb01e165f3fd5875244f33c9a0",
            "bin/toolkit-package-inspect": "15dc10d73ae14877dfc7fffe4c11d44d6f7742a7f8f91913b184f33339de9c04",
        },
        "provenance_trees": ("provenance/source/", "provenance/node-linux/"),
        "provenance_files": ("provenance/build.json", "provenance/build.log"),
    },
}
# Exact feature-enabled Linux build closure and its original dependency notices.
PYTHON_KITS["linux-x86_64"]["sources"].update({
    "selected/contracts/MCPB-ACQUISITION.md": ("sdk/consumer-setup/MCPB-ACQUISITION.md", "2fc530ea03b93d708fccae7c25ef6480481a7a1fe6c013d146e6f1010f726041"),
    'selected/provenance/linux-mcpb/Cargo.lock': ('docs/verification/20261008-linux-mcpb/provenance/Cargo.lock', 'f911a612fd54bd1634e9a1d36a955198723779c2fcd43efded9acb82885f357a'),
    'selected/provenance/linux-mcpb/build-result.json': ('docs/verification/20261008-linux-mcpb/provenance/build-result.json', '8e05aac4750d4772cdf5180a29de7ec011303f29c67aa23708f186914b3ce0e1'),
    'selected/provenance/linux-mcpb/build-state.json': ('docs/verification/20261008-linux-mcpb/provenance/build-state.json', '64c515c9cdf31d8659887ab815b98185246d968dab3c91f6a6db8cb164c52106'),
    'selected/provenance/linux-mcpb/build.stderr': ('docs/verification/20261008-linux-mcpb/provenance/build.stderr', '8a85b8aeb59ce5dfe2e4ff41ee0e1bc68d6fb267b849ece760e4c2b93feba6ae'),
    'selected/provenance/linux-mcpb/build.stdout': ('docs/verification/20261008-linux-mcpb/provenance/build.stdout', '7b36fcfeedaa3a5935386cac1999bb0d4cf9bdc6fc13605312d978565daab127'),
    'selected/provenance/linux-mcpb/cargo.txt': ('docs/verification/20261008-linux-mcpb/provenance/cargo.txt', '9046ebb981a850fd1c3cef9bbc73aa7cea82ec651754a30e4c38b842b7f91f7a'),
    'selected/provenance/linux-mcpb/compiler-artifacts.json': ('docs/verification/20261008-linux-mcpb/provenance/compiler-artifacts.json', 'd3b933981d269dbffdc28d34e9f866861f4078fd8f2403f77480572f29be1288'),
    'selected/provenance/linux-mcpb/compiler-os.txt': ('docs/verification/20261008-linux-mcpb/provenance/compiler-os.txt', '59a77b5f2666d9c85c489bd1911a6eebbd91ef22fe48b90a3b75f1b21f3844d4'),
    'selected/provenance/linux-mcpb/crates/adler2-2.0.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/adler2-2.0.1.crate', '320119579fcad9c21884f5c4861d16174d0e06250625266f50fe6898340abefa'),
    'selected/provenance/linux-mcpb/crates/ambient-authority-0.0.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/ambient-authority-0.0.2.crate', 'e9d4ee0d472d1cd2e28c97dfa124b3d8d992e10eb0a035f33f5d12e3a177ba3b'),
    'selected/provenance/linux-mcpb/crates/bitflags-2.13.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/bitflags-2.13.2.crate', '3ded4057c258ba199e2d26386d3af3780957ecaee6c4ef4041c6b4b8b97c0b06'),
    'selected/provenance/linux-mcpb/crates/block-buffer-0.10.4.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/block-buffer-0.10.4.crate', '3078c7629b62d3f0439517fa394996acacc5cbc91c5a20d8c658e77abd503a71'),
    'selected/provenance/linux-mcpb/crates/cap-primitives-4.0.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/cap-primitives-4.0.3.crate', '8b5f74729fd2f44701d1a8eb47e906cdb3ccd9ec0f02baad85a744b791940b18'),
    'selected/provenance/linux-mcpb/crates/cap-std-4.0.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/cap-std-4.0.3.crate', 'c1ec78e242cfa2cfe276807ac2ecc00315a6c97786977414bcd1c3963b6c91b8'),
    'selected/provenance/linux-mcpb/crates/cfg-if-1.0.5.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/cfg-if-1.0.5.crate', '4e7648175b45a9a48536d676f68d918270699102aa8dab5496df06904c914600'),
    'selected/provenance/linux-mcpb/crates/cpufeatures-0.2.17.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/cpufeatures-0.2.17.crate', '59ed5838eebb26a2bb2e58f6d5b5316989ae9d08bab10e0e6d103e656d1b0280'),
    'selected/provenance/linux-mcpb/crates/crc32fast-1.5.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/crc32fast-1.5.2.crate', '01a7799fd6b852db0e61728dde9a204c423b44d689dbd432522543614b490e78'),
    'selected/provenance/linux-mcpb/crates/crypto-common-0.1.7.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/crypto-common-0.1.7.crate', '78c8292055d1c1df0cce5d180393dc8cce0abec0a7102adb6c7b1eef6016d60a'),
    'selected/provenance/linux-mcpb/crates/digest-0.10.7.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/digest-0.10.7.crate', '9ed9a281f7bc9b7576e61468ba615a66a5c8cfdff42420a70aa82701a3b1e292'),
    'selected/provenance/linux-mcpb/crates/displaydoc-0.2.7.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/displaydoc-0.2.7.crate', 'c6232dd377dcc64799954cbd3a9bb882e9cdc1308ccd87b1c098f1fb2eaf82a8'),
    'selected/provenance/linux-mcpb/crates/equivalent-1.0.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/equivalent-1.0.2.crate', '877a4ace8713b0bcf2a4e7eec82529c029f1d0619886d18145fea96c3ffe5c0f'),
    'selected/provenance/linux-mcpb/crates/flate2-1.1.10.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/flate2-1.1.10.crate', '6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb'),
    'selected/provenance/linux-mcpb/crates/form_urlencoded-1.2.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/form_urlencoded-1.2.2.crate', 'cb4cb245038516f5f85277875cdaa4f7d2c9a0fa0468de06ed190163b1581fcf'),
    'selected/provenance/linux-mcpb/crates/fs-set-times-0.20.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/fs-set-times-0.20.3.crate', '94e7099f6313ecacbe1256e8ff9d617b75d1bcb16a6fddef94866d225a01a14a'),
    'selected/provenance/linux-mcpb/crates/generic-array-0.14.7.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/generic-array-0.14.7.crate', '85649ca51fd72272d7821adaf274ad91c288277713d9c18820d8499a7ff69e9a'),
    'selected/provenance/linux-mcpb/crates/hashbrown-0.17.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/hashbrown-0.17.1.crate', 'ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a'),
    'selected/provenance/linux-mcpb/crates/hex-0.4.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/hex-0.4.3.crate', '7f24254aa9a54b5c858eaee2f5bccdb46aaf0e486a595ed5fd8f86ba55232a70'),
    'selected/provenance/linux-mcpb/crates/icu_collections-2.3.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/icu_collections-2.3.0.crate', 'fa68d21081c4a05d5a901a1c62add574c77048b6a1c67be3b50ce0b60d4ca513'),
    'selected/provenance/linux-mcpb/crates/icu_locale_core-2.3.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/icu_locale_core-2.3.0.crate', 'd56e28588da92eee5c3201a6eff33fabdd49b62269c8938d4ff050ce4d900deb'),
    'selected/provenance/linux-mcpb/crates/icu_normalizer-2.3.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/icu_normalizer-2.3.0.crate', '12f9cf5f235641ed274641dd81c3f28d870e276763d0797aeeab72317b1c646f'),
    'selected/provenance/linux-mcpb/crates/icu_normalizer_data-2.3.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/icu_normalizer_data-2.3.0.crate', '1563da1ed3e0b3bf3d74c9b85917ac9c56464d2f57242270c09c9e752f8021a0'),
    'selected/provenance/linux-mcpb/crates/icu_properties-2.3.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/icu_properties-2.3.0.crate', '7e7ca276ad3145661a65914e6daf131ca5120cd3dcee8f8f3214b8875184a148'),
    'selected/provenance/linux-mcpb/crates/icu_properties_data-2.3.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/icu_properties_data-2.3.0.crate', 'e590f038c1464a96894fd6d10127e90a8be4509f56ff7ecef851b15cee0b7caa'),
    'selected/provenance/linux-mcpb/crates/icu_provider-2.3.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/icu_provider-2.3.1.crate', 'd27bbb9d3abbefac45d55f647c9de1d44aafcd1186eb91879afef17c396c3e73'),
    'selected/provenance/linux-mcpb/crates/idna-1.1.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/idna-1.1.0.crate', '3b0875f23caa03898994f6ddc501886a45c7d3d62d04d2d90788d47be1b1e4de'),
    'selected/provenance/linux-mcpb/crates/idna_adapter-1.2.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/idna_adapter-1.2.2.crate', 'cb68373c0d6620ef8105e855e7745e18b0d00d3bdb07fb532e434244cdb9a714'),
    'selected/provenance/linux-mcpb/crates/indexmap-2.14.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/indexmap-2.14.2.crate', 'cc4e190f5d26ca7051642629da2c52fc03bde85a03197c99408dcd291734c855'),
    'selected/provenance/linux-mcpb/crates/io-extras-0.19.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/io-extras-0.19.0.crate', '20fd6de4ccfcc187e38bc21cfa543cb5a302cb86a8b114eb7f0bf0dc9f8ac00f'),
    'selected/provenance/linux-mcpb/crates/io-lifetimes-2.0.4.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/io-lifetimes-2.0.4.crate', '06432fb54d3be7964ecd3649233cddf80db2832f47fec34c01f65b3d9d774983'),
    'selected/provenance/linux-mcpb/crates/io-lifetimes-3.0.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/io-lifetimes-3.0.1.crate', '2f0fb0570afe1fed943c5c3d4102d5358592d8625fda6a0007fdbe65a92fba96'),
    'selected/provenance/linux-mcpb/crates/ipnet-2.12.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/ipnet-2.12.2.crate', '791930b43c0d5973160d90a8f3894509f2b273430f5c5c73b668636d0287c5c0'),
    'selected/provenance/linux-mcpb/crates/itoa-1.0.18.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/itoa-1.0.18.crate', '8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682'),
    'selected/provenance/linux-mcpb/crates/libc-0.2.189.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/libc-0.2.189.crate', '3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2'),
    'selected/provenance/linux-mcpb/crates/linux-raw-sys-0.12.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/linux-raw-sys-0.12.1.crate', '32a66949e030da00e8c7d4434b251670a91556f4144941d37452769c25d58a53'),
    'selected/provenance/linux-mcpb/crates/litemap-0.8.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/litemap-0.8.3.crate', '47d9d19d1d6efa0109d2f65ff4c85cddd50bd572e5a00127ab10987290bcefae'),
    'selected/provenance/linux-mcpb/crates/maybe-owned-0.3.4.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/maybe-owned-0.3.4.crate', '4facc753ae494aeb6e3c22f839b158aebd4f9270f55cd3c79906c45476c47ab4'),
    'selected/provenance/linux-mcpb/crates/memchr-2.8.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/memchr-2.8.3.crate', 'cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98'),
    'selected/provenance/linux-mcpb/crates/miniz_oxide-0.9.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/miniz_oxide-0.9.1.crate', 'b63fbc4a50860e98e7b2aa7804ded1db5cbc3aff9193adaff57a6931bf7c4b4c'),
    'selected/provenance/linux-mcpb/crates/once_cell-1.21.4.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/once_cell-1.21.4.crate', '9f7c3e4beb33f85d45ae3e3a1792185706c8e16d043238c593331cc7cd313b50'),
    'selected/provenance/linux-mcpb/crates/percent-encoding-2.3.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/percent-encoding-2.3.2.crate', '9b4f627cb1b25917193a259e49bdad08f671f8d9708acfd5fe0a8c1455d87220'),
    'selected/provenance/linux-mcpb/crates/potential_utf-0.1.6.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/potential_utf-0.1.6.crate', 'd83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661'),
    'selected/provenance/linux-mcpb/crates/proc-macro2-1.0.107.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/proc-macro2-1.0.107.crate', '985e7ec9bb745e6ce6535b544d84d6cd6f7ad8bd711c398938ae983b91a766d9'),
    'selected/provenance/linux-mcpb/crates/quote-1.0.47.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/quote-1.0.47.crate', '1fbf4db142a473a8d80c26bbf18454ed458bf8d26c8219c331daecfdbd079001'),
    'selected/provenance/linux-mcpb/crates/rustix-1.1.5.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/rustix-1.1.5.crate', '891efababe418670775f199f0d233d84843c227a0949a883ce15b37c78d6629d'),
    'selected/provenance/linux-mcpb/crates/rustix-linux-procfs-0.1.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/rustix-linux-procfs-0.1.1.crate', '2fc84bf7e9aa16c4f2c758f27412dc9841341e16aa682d9c7ac308fe3ee12056'),
    'selected/provenance/linux-mcpb/crates/serde-1.0.229.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/serde-1.0.229.crate', '4148590afebada386688f18773da617792bf2ef03ffc1e4cbd2b1d45b023e0ba'),
    'selected/provenance/linux-mcpb/crates/serde_core-1.0.229.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/serde_core-1.0.229.crate', '67dca2c9c51e58a4791a4b1ed58308b39c64224d349a935ab5039aa360942a48'),
    'selected/provenance/linux-mcpb/crates/serde_derive-1.0.229.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/serde_derive-1.0.229.crate', 'e7a5d71263a5a7d47b41f6b3f06ba276f10cc18b0931f1799f710578e2309348'),
    'selected/provenance/linux-mcpb/crates/serde_json-1.0.151.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/serde_json-1.0.151.crate', 'c841b55ecdae098c80dcae9cf767f6f8a0c2cdb3416bbef72181df4d0fe73f14'),
    'selected/provenance/linux-mcpb/crates/serde_path_to_error-0.1.20.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/serde_path_to_error-0.1.20.crate', '10a9ff822e371bb5403e391ecd83e182e0e77ba7f6fe0160b795797109d1b457'),
    'selected/provenance/linux-mcpb/crates/sha2-0.10.9.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/sha2-0.10.9.crate', 'a7507d819769d01a365ab707794a4084392c824f54a7a6a7862f8c3d0892b283'),
    'selected/provenance/linux-mcpb/crates/simd-adler32-0.3.10.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/simd-adler32-0.3.10.crate', '3a219298ac11a56ea9a6d2120044824d6f01aeb034955e7af7bc16858527deea'),
    'selected/provenance/linux-mcpb/crates/smallvec-1.16.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/smallvec-1.16.2.crate', 'f9395f0f0eee849a9b707b2f06bb92a6a422090e2123bb2ef8e87a0e61892a8e'),
    'selected/provenance/linux-mcpb/crates/stable_deref_trait-1.2.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/stable_deref_trait-1.2.1.crate', '6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596'),
    'selected/provenance/linux-mcpb/crates/syn-3.0.6.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/syn-3.0.6.crate', '8593e8e72159ed2257d083c7a454a85cbf854f37a0966d8d483aff8c8a3ebcee'),
    'selected/provenance/linux-mcpb/crates/synstructure-0.14.0.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/synstructure-0.14.0.crate', '901704edd0dfe137f1987838ee4f259e4e063c31371bdb423f7ae38ec6f77f02'),
    'selected/provenance/linux-mcpb/crates/thiserror-2.0.21.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/thiserror-2.0.21.crate', '09e52cb86a36cede5cb101bf8908837b3e4c6e5e59fe7fd85c23fb56200d189e'),
    'selected/provenance/linux-mcpb/crates/thiserror-impl-2.0.21.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/thiserror-impl-2.0.21.crate', 'fe5197923287db20a58125f0bc85c062f7f2c892de97b18c356f9efb14b28524'),
    'selected/provenance/linux-mcpb/crates/tinystr-0.8.4.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/tinystr-0.8.4.crate', 'b1e27c91459209c2986af3dcf603a5a74a4368754ce37414f59acc971167f643'),
    'selected/provenance/linux-mcpb/crates/typenum-1.20.1.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/typenum-1.20.1.crate', 'b6f5e870be6c3b371b77fe0ee0bafb859fa4964b4404c27de1d380043c4dda20'),
    'selected/provenance/linux-mcpb/crates/unicode-ident-1.0.26.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/unicode-ident-1.0.26.crate', 'd245f478577f809a851594d02313b640fb437e0bb33866753cff937863096954'),
    'selected/provenance/linux-mcpb/crates/url-2.5.8.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/url-2.5.8.crate', 'ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed'),
    'selected/provenance/linux-mcpb/crates/utf8_iter-1.0.4.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/utf8_iter-1.0.4.crate', 'b6c140620e7ffbb22c2dee59cafe6084a59b5ffc27a8859a5f0d494b5d52b6be'),
    'selected/provenance/linux-mcpb/crates/version_check-0.9.5.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/version_check-0.9.5.crate', '0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a'),
    'selected/provenance/linux-mcpb/crates/writeable-0.6.4.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/writeable-0.6.4.crate', '3ad82d2a33cdc9674dc7465672f271e096168fcdbe0f799d9e6db8c5892679dc'),
    'selected/provenance/linux-mcpb/crates/yoke-0.8.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/yoke-0.8.3.crate', '709fe23a0424b6a435d82152b1bd3fdfb0833487d5fa90d05d42762a9891fef5'),
    'selected/provenance/linux-mcpb/crates/yoke-derive-0.8.3.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/yoke-derive-0.8.3.crate', '33811428bee40dbceb6d545e95754741d17a6aef9a4849f0fd62e2ba4f412a78'),
    'selected/provenance/linux-mcpb/crates/zerofrom-0.1.8.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/zerofrom-0.1.8.crate', '0ec05a11813ea801ff6d75110ad09cd0824ddba17dfe17128ea0d5f68e6c5272'),
    'selected/provenance/linux-mcpb/crates/zerofrom-derive-0.1.8.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/zerofrom-derive-0.1.8.crate', 'f75b4683f6c7f45248d4d64056a24298c6281e0993356d7d1b4a1a962ef10d4a'),
    'selected/provenance/linux-mcpb/crates/zerotrie-0.2.5.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/zerotrie-0.2.5.crate', '4ea269c3bd32f0a32c321907a2ae912ba6f4649bb0fc764a15627e99a7095a3f'),
    'selected/provenance/linux-mcpb/crates/zerovec-0.11.8.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/zerovec-0.11.8.crate', 'bb0464e17806c1d976d5cba29399c7f08e516e279e2ba493f63123b5fca67dd8'),
    'selected/provenance/linux-mcpb/crates/zerovec-derive-0.11.6.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/zerovec-derive-0.11.6.crate', '34df6fc39dbd26ddc9c10e6a2984476e13acce22e64e4487636ef494369225da'),
    'selected/provenance/linux-mcpb/crates/zip-2.4.2.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/zip-2.4.2.crate', 'fabe6324e908f85a1c52063ce7aa26b68dcb7eb6dbc83a2d148403c9bc3eba50'),
    'selected/provenance/linux-mcpb/crates/zmij-1.0.23.crate': ('docs/verification/20261008-linux-mcpb/provenance/crates/zmij-1.0.23.crate', '29666d0abbfad1e3dc4dcf6144730dd3a3ab225bbbdac83319345b1b44ccfc1b'),
    'selected/provenance/linux-mcpb/dependencies.json': ('docs/verification/20261008-linux-mcpb/provenance/dependencies.json', 'afa29a04d88b0ae7a43c0acc424a718ff942c713c051d0b8dbb44d394bf8ccb2'),
    'selected/provenance/linux-mcpb/inspector-elf-versions.txt': ('docs/verification/20261008-linux-mcpb/provenance/inspector-elf-versions.txt', '6ef43b80796e607aee5a4ce5ace5086feac15dfa3e4d9a03874f75ca2fae7094'),
    'selected/provenance/linux-mcpb/inspector-ldd.txt': ('docs/verification/20261008-linux-mcpb/provenance/inspector-ldd.txt', '069beb8cbe6dd88671cb25729353b95315c8ea5867546cc8277b51b205e4b089'),
    'selected/provenance/linux-mcpb/installer-elf-versions.txt': ('docs/verification/20261008-linux-mcpb/provenance/installer-elf-versions.txt', 'b63eb7e522d541221f6d874b8787e5892b29892bef453094143a52efa72a37e1'),
    'selected/provenance/linux-mcpb/installer-ldd.txt': ('docs/verification/20261008-linux-mcpb/provenance/installer-ldd.txt', '95ef53e8b7c0ff9e4263db8f57cadd4fd7f8f3f2a01d62051fd7ceff7423b5fb'),
    'selected/licenses/linux-mcpb/adler2-2.0.1/LICENSE-0BSD': ('docs/verification/20261008-linux-mcpb/provenance/licenses/adler2-2.0.1/LICENSE-0BSD', '861399f8c21c042b110517e76dc6b63a2b334276c8cf17412fc3c8908ca8dc17'),
    'selected/licenses/linux-mcpb/adler2-2.0.1/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/adler2-2.0.1/LICENSE-APACHE', '8ada45cd9f843acf64e4722ae262c622a2b3b3007c7310ef36ac1061a30f6adb'),
    'selected/licenses/linux-mcpb/adler2-2.0.1/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/adler2-2.0.1/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/ambient-authority-0.0.2/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/ambient-authority-0.0.2/COPYRIGHT', 'd00d6d48df2db6f91e4ca9c656e5e30782d5becf52bb3e5bfd0aff467bd37781'),
    'selected/licenses/linux-mcpb/ambient-authority-0.0.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/ambient-authority-0.0.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/ambient-authority-0.0.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/ambient-authority-0.0.2/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/bitflags-2.13.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/bitflags-2.13.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/bitflags-2.13.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/bitflags-2.13.2/LICENSE-MIT', '6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb'),
    'selected/licenses/linux-mcpb/block-buffer-0.10.4/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/block-buffer-0.10.4/LICENSE-APACHE', 'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
    'selected/licenses/linux-mcpb/block-buffer-0.10.4/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/block-buffer-0.10.4/LICENSE-MIT', 'd5c22aa3118d240e877ad41c5d9fa232f9c77d757d4aac0c2f943afc0a95e0ef'),
    'selected/licenses/linux-mcpb/cap-primitives-4.0.3/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-primitives-4.0.3/COPYRIGHT', 'a8799c2a07dabcb8d52481f885c5ab7000acb8a4d3cd9503fbcb97c84d350e65'),
    'selected/licenses/linux-mcpb/cap-primitives-4.0.3/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-primitives-4.0.3/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/cap-primitives-4.0.3/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-primitives-4.0.3/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/cap-std-4.0.3/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-std-4.0.3/COPYRIGHT', '6b1691fdee3b03c3946696a7be125fcf50eff310b9486fac850c1b8aea91478b'),
    'selected/licenses/linux-mcpb/cap-std-4.0.3/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-std-4.0.3/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/cap-std-4.0.3/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cap-std-4.0.3/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/cfg-if-1.0.5/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cfg-if-1.0.5/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/cfg-if-1.0.5/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cfg-if-1.0.5/LICENSE-MIT', '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
    'selected/licenses/linux-mcpb/cpufeatures-0.2.17/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cpufeatures-0.2.17/LICENSE-APACHE', 'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
    'selected/licenses/linux-mcpb/cpufeatures-0.2.17/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/cpufeatures-0.2.17/LICENSE-MIT', 'ae9baa7beea910273c2f384c2a6b721fb7bd02bda3436074a1072e4ee689f985'),
    'selected/licenses/linux-mcpb/crc32fast-1.5.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/crc32fast-1.5.2/LICENSE-APACHE', 'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
    'selected/licenses/linux-mcpb/crc32fast-1.5.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/crc32fast-1.5.2/LICENSE-MIT', '61d383b05b87d78f94d2937e2580cce47226d17823c0430fbcad09596537efcf'),
    'selected/licenses/linux-mcpb/crypto-common-0.1.7/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/crypto-common-0.1.7/LICENSE-APACHE', 'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
    'selected/licenses/linux-mcpb/crypto-common-0.1.7/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/crypto-common-0.1.7/LICENSE-MIT', '3521672491a3479422d5fe1aca6645dd2984090f85da6e5205abfb18fb7a6897'),
    'selected/licenses/linux-mcpb/digest-0.10.7/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/digest-0.10.7/LICENSE-APACHE', 'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
    'selected/licenses/linux-mcpb/digest-0.10.7/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/digest-0.10.7/LICENSE-MIT', '9e0dfd2dd4173a530e238cb6adb37aa78c34c6bc7444e0e10c1ab5d8881f63ba'),
    'selected/licenses/linux-mcpb/displaydoc-0.2.7/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/displaydoc-0.2.7/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/displaydoc-0.2.7/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/displaydoc-0.2.7/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/equivalent-1.0.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/equivalent-1.0.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/equivalent-1.0.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/equivalent-1.0.2/LICENSE-MIT', '7365cc8878a1d7ce155a58c4ca09c3d7a6be413efa5334a80ea842912b669349'),
    'selected/licenses/linux-mcpb/flate2-1.1.10/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/flate2-1.1.10/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/flate2-1.1.10/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/flate2-1.1.10/LICENSE-MIT', '025436edff4cfcdde17a5811fdea78892d8482efd1abdec5a17872d07a4f2112'),
    'selected/licenses/linux-mcpb/form_urlencoded-1.2.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/form_urlencoded-1.2.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/form_urlencoded-1.2.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/form_urlencoded-1.2.2/LICENSE-MIT', '20c7855c364d57ea4c97889a5e8d98470a9952dade37bd9248b9a54431670e5e'),
    'selected/licenses/linux-mcpb/fs-set-times-0.20.3/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/fs-set-times-0.20.3/COPYRIGHT', 'e196ffeb5ab101e9ec8bbe395c9f367c5380ee2d28c47e5c03371523798a9af7'),
    'selected/licenses/linux-mcpb/fs-set-times-0.20.3/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/fs-set-times-0.20.3/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/fs-set-times-0.20.3/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/fs-set-times-0.20.3/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/generic-array-0.14.7/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/generic-array-0.14.7/LICENSE', 'c09aae9d3c77b531f56351a9947bc7446511d6b025b3255312d3e3442a9a7583'),
    'selected/licenses/linux-mcpb/hashbrown-0.17.1/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/hashbrown-0.17.1/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/hashbrown-0.17.1/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/hashbrown-0.17.1/LICENSE-MIT', 'ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2'),
    'selected/licenses/linux-mcpb/hex-0.4.3/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/hex-0.4.3/LICENSE-APACHE', 'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
    'selected/licenses/linux-mcpb/hex-0.4.3/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/hex-0.4.3/LICENSE-MIT', 'f7bdb3426d045cd50efd4953026e3eb5a83d0199f458a075602611b9344da5b9'),
    'selected/licenses/linux-mcpb/icu_collections-2.3.0/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/icu_collections-2.3.0/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/icu_locale_core-2.3.0/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/icu_locale_core-2.3.0/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/icu_normalizer-2.3.0/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/icu_normalizer-2.3.0/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/icu_normalizer_data-2.3.0/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/icu_normalizer_data-2.3.0/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/icu_properties-2.3.0/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/icu_properties-2.3.0/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/icu_properties_data-2.3.0/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/icu_properties_data-2.3.0/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/icu_provider-2.3.1/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/icu_provider-2.3.1/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/idna-1.1.0/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/idna-1.1.0/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/idna-1.1.0/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/idna-1.1.0/LICENSE-MIT', 'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
    'selected/licenses/linux-mcpb/idna_adapter-1.2.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/idna_adapter-1.2.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/idna_adapter-1.2.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/idna_adapter-1.2.2/LICENSE-MIT', '8b43ce8accd61e9d370b5ca9e9c4f953279b5c239926c62315b40e24df51b726'),
    'selected/licenses/linux-mcpb/indexmap-2.14.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/indexmap-2.14.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/indexmap-2.14.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/indexmap-2.14.2/LICENSE-MIT', 'ecc269ef87fd38a1d98e30bfac9ba964a9dbd9315c3770fed98d4d7cb5882055'),
    'selected/licenses/linux-mcpb/io-extras-0.19.0/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-extras-0.19.0/COPYRIGHT', 'f0c1d22de8bd3b776a40f2691f08b84e91c7bafc78b72a51150e231002b88db3'),
    'selected/licenses/linux-mcpb/io-extras-0.19.0/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-extras-0.19.0/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/io-extras-0.19.0/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-extras-0.19.0/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/io-lifetimes-2.0.4/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-2.0.4/COPYRIGHT', '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
    'selected/licenses/linux-mcpb/io-lifetimes-2.0.4/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-2.0.4/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/io-lifetimes-2.0.4/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-2.0.4/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/io-lifetimes-3.0.1/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-3.0.1/COPYRIGHT', '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
    'selected/licenses/linux-mcpb/io-lifetimes-3.0.1/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-3.0.1/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/io-lifetimes-3.0.1/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/io-lifetimes-3.0.1/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/ipnet-2.12.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/ipnet-2.12.2/LICENSE-APACHE', '87d9feb9238c6bd8e0024fc4733b06cff036f89f36d93b7df1c8a0549bbb7a5b'),
    'selected/licenses/linux-mcpb/ipnet-2.12.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/ipnet-2.12.2/LICENSE-MIT', '47dc9ff29128ddfb4d6a0435383c9f89120bc374dbcc1dd00b933a0b28aa7865'),
    'selected/licenses/linux-mcpb/itoa-1.0.18/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/itoa-1.0.18/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/itoa-1.0.18/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/itoa-1.0.18/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/libc-0.2.189/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/libc-0.2.189/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/libc-0.2.189/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/libc-0.2.189/LICENSE-MIT', '123a331b5dbf04c30097fa43b8f858bc85df671fe776de498d01f3d6b7c1f69e'),
    'selected/licenses/linux-mcpb/linux-raw-sys-0.12.1/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/linux-raw-sys-0.12.1/COPYRIGHT', '3290ae0fbc9ddb77d2239121d710f0bb9d31b3b4744e6d97fe01e652b4c1870b'),
    'selected/licenses/linux-mcpb/linux-raw-sys-0.12.1/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/linux-raw-sys-0.12.1/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/litemap-0.8.3/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/litemap-0.8.3/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/maybe-owned-0.3.4/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/maybe-owned-0.3.4/LICENSE-APACHE', 'aecc100d0547aa3bcbe04bcf5cb87c7db0be11004602adfa5056edf8bb847b35'),
    'selected/licenses/linux-mcpb/maybe-owned-0.3.4/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/maybe-owned-0.3.4/LICENSE-MIT', 'ef2b3bbd7b718a7881253226313f103e89b379e1279681a3c1ebcb65a27d209d'),
    'selected/licenses/linux-mcpb/memchr-2.8.3/COPYING': ('docs/verification/20261008-linux-mcpb/provenance/licenses/memchr-2.8.3/COPYING', '01c266bced4a434da0051174d6bee16a4c82cf634e2679b6155d40d75012390f'),
    'selected/licenses/linux-mcpb/memchr-2.8.3/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/memchr-2.8.3/LICENSE-MIT', '0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f'),
    'selected/licenses/linux-mcpb/miniz_oxide-0.9.1/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/miniz_oxide-0.9.1/LICENSE', '4108245a1f2df9d4e94df8abed5b4ba0759bb2f9b40a6b939f1be141077ae50b'),
    'selected/licenses/linux-mcpb/miniz_oxide-0.9.1/LICENSE-APACHE.md': ('docs/verification/20261008-linux-mcpb/provenance/licenses/miniz_oxide-0.9.1/LICENSE-APACHE.md', '0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594'),
    'selected/licenses/linux-mcpb/miniz_oxide-0.9.1/LICENSE-MIT.md': ('docs/verification/20261008-linux-mcpb/provenance/licenses/miniz_oxide-0.9.1/LICENSE-MIT.md', '799e9ca9d179295ef372f25d3769cdda7d25bb2668add6a6a1e22d1e4c678b8d'),
    'selected/licenses/linux-mcpb/miniz_oxide-0.9.1/LICENSE-ZLIB.md': ('docs/verification/20261008-linux-mcpb/provenance/licenses/miniz_oxide-0.9.1/LICENSE-ZLIB.md', '0a54e647fe54104658b5e563c04c6f9edf251710e47bce692e0bd990a4ddaa39'),
    'selected/licenses/linux-mcpb/once_cell-1.21.4/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/once_cell-1.21.4/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/once_cell-1.21.4/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/once_cell-1.21.4/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/percent-encoding-2.3.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/percent-encoding-2.3.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/percent-encoding-2.3.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/percent-encoding-2.3.2/LICENSE-MIT', 'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
    'selected/licenses/linux-mcpb/potential_utf-0.1.6/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/potential_utf-0.1.6/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/proc-macro2-1.0.107/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/proc-macro2-1.0.107/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/proc-macro2-1.0.107/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/proc-macro2-1.0.107/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/quote-1.0.47/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/quote-1.0.47/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/quote-1.0.47/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/quote-1.0.47/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/rustix-1.1.5/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-1.1.5/COPYRIGHT', '377c2e7c53250cc5905c0b0532d35973392af16ffb9596a41d99d202cf3617c9'),
    'selected/licenses/linux-mcpb/rustix-1.1.5/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-1.1.5/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/rustix-1.1.5/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-1.1.5/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/rustix-linux-procfs-0.1.1/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-linux-procfs-0.1.1/COPYRIGHT', 'd415a86ccfd794129666247cbbadebee7a6d0ac3ff701c76c07b0f9424f0307d'),
    'selected/licenses/linux-mcpb/rustix-linux-procfs-0.1.1/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception', '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
    'selected/licenses/linux-mcpb/rustix-linux-procfs-0.1.1/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/serde-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde-1.0.229/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/serde-1.0.229/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde-1.0.229/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/serde_core-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_core-1.0.229/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/serde_core-1.0.229/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_core-1.0.229/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/serde_derive-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_derive-1.0.229/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/serde_derive-1.0.229/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_derive-1.0.229/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/serde_json-1.0.151/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_json-1.0.151/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/serde_json-1.0.151/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_json-1.0.151/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/serde_path_to_error-0.1.20/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_path_to_error-0.1.20/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/serde_path_to_error-0.1.20/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/serde_path_to_error-0.1.20/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/sha2-0.10.9/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/sha2-0.10.9/LICENSE-APACHE', 'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
    'selected/licenses/linux-mcpb/sha2-0.10.9/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/sha2-0.10.9/LICENSE-MIT', 'b4eb00df6e2a4d22518fcaa6a2b4646f249b3a3c9814509b22bd2091f1392ff1'),
    'selected/licenses/linux-mcpb/simd-adler32-0.3.10/LICENSE.md': ('docs/verification/20261008-linux-mcpb/provenance/licenses/simd-adler32-0.3.10/LICENSE.md', '42a35170233e83e18856792e748de4c1ce4a63b2afce9a370c89ef3fe23f9f2d'),
    'selected/licenses/linux-mcpb/smallvec-1.16.2/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/smallvec-1.16.2/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/smallvec-1.16.2/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/smallvec-1.16.2/LICENSE-MIT', '0b28172679e0009b655da42797c03fd163a3379d5cfa67ba1f1655e974a2a1a9'),
    'selected/licenses/linux-mcpb/stable_deref_trait-1.2.1/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/stable_deref_trait-1.2.1/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/stable_deref_trait-1.2.1/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/stable_deref_trait-1.2.1/LICENSE-MIT', '5e05b024f653a5ce199e77cbbbd42fb5553562ec714b819421ed0c3e552a75d7'),
    'selected/licenses/linux-mcpb/syn-3.0.6/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/syn-3.0.6/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/syn-3.0.6/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/syn-3.0.6/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/synstructure-0.14.0/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/synstructure-0.14.0/LICENSE', '219920e865eee70b7dcfc948a86b099e7f4fe2de01bcca2ca9a20c0a033f2b59'),
    'selected/licenses/linux-mcpb/thiserror-2.0.21/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/thiserror-2.0.21/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/thiserror-2.0.21/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/thiserror-2.0.21/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/thiserror-impl-2.0.21/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/thiserror-impl-2.0.21/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/thiserror-impl-2.0.21/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/thiserror-impl-2.0.21/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/tinystr-0.8.4/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/tinystr-0.8.4/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/typenum-1.20.1/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/typenum-1.20.1/LICENSE', 'db11fec9946737df39ca3898d9cd8c10ec6f6c3a884a6802b0ad0b81b4e8f23a'),
    'selected/licenses/linux-mcpb/typenum-1.20.1/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/typenum-1.20.1/LICENSE-APACHE', '516b24e051bf5630880ebbd55c40a25ce9552ebaf8970a53e8976eb70e522406'),
    'selected/licenses/linux-mcpb/typenum-1.20.1/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/typenum-1.20.1/LICENSE-MIT', 'a825bd853ab71619a4923d7b4311221427848070ff44d990da39b0b274c1683f'),
    'selected/licenses/linux-mcpb/unicode-ident-1.0.26/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/unicode-ident-1.0.26/LICENSE-APACHE', '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
    'selected/licenses/linux-mcpb/unicode-ident-1.0.26/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/unicode-ident-1.0.26/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/licenses/linux-mcpb/unicode-ident-1.0.26/LICENSE-UNICODE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/unicode-ident-1.0.26/LICENSE-UNICODE', 'f7db81051789b729fea528a63ec4c938fdcb93d9d61d97dc8cc2e9df6d47f2a1'),
    'selected/licenses/linux-mcpb/url-2.5.8/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/url-2.5.8/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/url-2.5.8/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/url-2.5.8/LICENSE-MIT', 'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
    'selected/licenses/linux-mcpb/utf8_iter-1.0.4/COPYRIGHT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/utf8_iter-1.0.4/COPYRIGHT', 'c30152c94a6d75e021adbc52b3a52470366a46edb917e17deae3259251af244c'),
    'selected/licenses/linux-mcpb/utf8_iter-1.0.4/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/utf8_iter-1.0.4/LICENSE-APACHE', 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'),
    'selected/licenses/linux-mcpb/utf8_iter-1.0.4/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/utf8_iter-1.0.4/LICENSE-MIT', '3fa4ca83dcc9237839b1bdeb2e6d16bdfb5ec0c5ce42b24694d8bbf0dcbef72c'),
    'selected/licenses/linux-mcpb/version_check-0.9.5/LICENSE-APACHE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/version_check-0.9.5/LICENSE-APACHE', 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
    'selected/licenses/linux-mcpb/version_check-0.9.5/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/version_check-0.9.5/LICENSE-MIT', 'b7e650f3fce5c53249d1cdc608b54df156a97edd636cf9d23498d0cfe7aec63e'),
    'selected/licenses/linux-mcpb/writeable-0.6.4/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/writeable-0.6.4/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/yoke-0.8.3/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/yoke-0.8.3/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/yoke-derive-0.8.3/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/yoke-derive-0.8.3/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/zerofrom-0.1.8/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/zerofrom-0.1.8/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/zerofrom-derive-0.1.8/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/zerofrom-derive-0.1.8/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/zerotrie-0.2.5/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/zerotrie-0.2.5/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/zerovec-0.11.8/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/zerovec-0.11.8/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/zerovec-derive-0.11.6/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/zerovec-derive-0.11.6/LICENSE', 'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
    'selected/licenses/linux-mcpb/zip-2.4.2/LICENSE': ('docs/verification/20261008-linux-mcpb/provenance/licenses/zip-2.4.2/LICENSE', '04dc52136d82400b11df5f474949863a70a5d05d2278dec45b99c479494450a6'),
    'selected/licenses/linux-mcpb/zmij-1.0.23/LICENSE-MIT': ('docs/verification/20261008-linux-mcpb/provenance/licenses/zmij-1.0.23/LICENSE-MIT', '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
    'selected/provenance/linux-mcpb/rustc.txt': ('docs/verification/20261008-linux-mcpb/provenance/rustc.txt', 'a90721a3b031e0e1c8bfc34ece79b6608d2ae53166cfb77dad281a0eb5fa6bc8'),
    'selected/provenance/linux-mcpb/source.tar.gz': ('docs/verification/20261008-linux-mcpb/provenance/source.tar.gz', '779b9f38367e49e4062cd4dd6a53c711e95d58437503ad5343df697147522fb5'),
    'selected/provenance/linux-mcpb/toolchain.sha256': ('docs/verification/20261008-linux-mcpb/provenance/toolchain.sha256', 'a9a439382571385cc2ab8438d5b253b1be668a011b688e994960605c03784342'),
    'selected/provenance/linux-mcpb/toolkit-mcpb-install-build.json': ('docs/verification/20261008-linux-mcpb/provenance/toolkit-mcpb-install-build.json', 'e0d444c93a76aea2a902d785f0b10532c0d6336d54ea72e9ad3ad62670851625'),
    'selected/provenance/linux-mcpb/toolkit-package-inspect-build.json': ('docs/verification/20261008-linux-mcpb/provenance/toolkit-package-inspect-build.json', '2552a44a8809ce155986ddae510d6e960b48610aa6ae66d02e1b44c550318399'),
    'selected/contracts/MCPB-INSTALL.md': ('crates/toolkit-package/MCPB-INSTALL.md', '3fe504efbd69f86c09d85bf9158d7ff958130aaa8d65648928d9f2179025a371'),
})

PYTHON_SOURCES = {
    "setup/public_github.py": ("sdk/consumer-setup/public_github.py", "e8aa82576a33e0083f31b60c17d383aa3e6bde7d4e9f1f517c301a543cf72b54"),
    'setup/compose_runtime.py': ('sdk/python/src/meerkat_toolkit/compose.py', '6bd686d0d2690cb6f6b1c473b99c600fd49ec95cff89f647ae499fc2ba5d6821'),
    'setup/compose_acquisition.py': ('sdk/consumer-setup/compose_acquisition.py', '9441fdc96dbfd8eb480d6d3c5277fdaf66ad359dd2b711d3cd3b819917c2349e'),
    'setup/compose_bridge.py': ('sdk/consumer-setup/compose_bridge.py', '48bfb800c21f322ee29c50e697a07d981856365f908ecb7b52e58ec028aa1257'),
    'selected/contracts/COMPOSE-ACQUISITION.md': ('sdk/consumer-setup/COMPOSE-ACQUISITION.md', '574ce12bd2df611af36a4b826fed94fe0086d0c6b31fb65b8d27b4785d8ba944'),
    'selected/contracts/COMPOSE.md': ('sdk/consumer-setup/COMPOSE.md', '0b5cc9358571f1bbc8b9467604d314b0fe2fe863c82f96ceff21b92b678d9898'),
    'selected/contracts/COMPOSE-PROPOSALS.md': ('sdk/python/COMPOSE-PROPOSALS.md', '78b016c94e8d2f5f6e3be98940f69d852478b235240fb305912209dc195130d6'),
    'selected/contracts/COMPOSE-RUNTIME.md': ('sdk/python/COMPOSE.md', '08fbe0810017f6591ec0f8a101401f88872eb4de66563485e6fa20a33e5c7ca3'),
    'selected/contracts/COMPOSE-CONTRACT.md': ('docs/verification/20261004-compose-lifecycle/canonical/CONTRACT.md', '04f9e674dfe8180e3c7f53d9686a3d6e17cb58d894a2b6bec5098965e201e00d'),
    'selected/contracts/COMPOSE-request.example.json': ('docs/verification/20261004-compose-lifecycle/canonical/request.example.json', '616d22d7318f903643d09e3ec58868b57cc48638ab812b139007432a4afbe6b3'),
    'selected/contracts/MCPB-ACQUISITION.md': ('sdk/consumer-setup/MCPB-ACQUISITION.md', '2fc530ea03b93d708fccae7c25ef6480481a7a1fe6c013d146e6f1010f726041'),
    'setup/mcpb_acquisition.py': ('sdk/consumer-setup/mcpb_acquisition.py', '5b3d074c0c8e84977602caf231bec07af7aa8bae8bcb9fe031d872f7739e8387'),
    'setup/bootstrap_host.py': ('sdk/consumer-setup/bootstrap_host.py', '06249f980076e1648109c7f6cf5e9ed647440a67d63ed59ea950c5874ca0cb56'),
    'selected/contracts/HOST-BOOTSTRAP.md': ('sdk/consumer-setup/HOST-BOOTSTRAP.md', 'dfeca2ea3f4e7b77b096ff03b9a332d7dabbbf4292dc343fc44845e3986b4331'),
    "selected/contracts/ACQUIRED-INSPECTION.md": ("sdk/consumer-setup/ACQUIRED-INSPECTION.md", "0e5c53aabf39a78197aa5572664940a1b73e3557a304fa00ea35e3189508f591"),
    "setup/acquired_inspection.py": ("sdk/consumer-setup/acquired_inspection.py", "2fa1b271c2f0627a56f582658ceb42ff7fccf8922a3f9dd50221d7ada4f0a67c"),
    "setup/node_acquisition.py": ("sdk/consumer-setup/node_acquisition.py", "ac54a5dcc7f7ddd4e1deeca9c8ca115f7a81e3da9a90e4031a4cb3caa0227949"),
    "setup/node_prebuilt_acquisition.py": ("sdk/consumer-setup/node_prebuilt_acquisition.py", "ebd591645466042c98689ba71d9d4c37978a7a9cd973f9ffd9b4cf2b0bbf2b48"),
    "selected/contracts/NODE-ACQUISITION.md": ("sdk/consumer-setup/NODE-ACQUISITION.md", "1083ccda5619b55e1055fa0969d56f9049fad4169aaebb9378dfc67456a3a35d"),
    "setup/local_session.py": ("sdk/consumer-setup/local_session.py", "79836ebe618fcf52efde781e1b5d3afdff45062f12c87761ce7eaeadee7c7ee2"),
    "setup/prepare_host.py": ("sdk/consumer-setup/prepare_host.py", "a31de9e408930c0e05ec8ec0994a0a879d546bdbdd16703439502295377895fa"),
    "selected/contracts/HOST-PREPARATION.md": ("sdk/consumer-setup/HOST-PREPARATION.md", "8a1aeb7b1d50c6ac9ba442608a5dbaf805dcc480c2738709f13fd41339b4e312"),
    "setup/python_acquisition.py": ("sdk/consumer-setup/python_acquisition.py", "7721011d0e67e9e66214913259eead0f4bd66e7c917dc26e452ffc944152503b"),
    "selected/agent/instructions.md": ("agents/setup-installer/instructions.md", "25464472904fcfc15544f7d30f529eed398694c9e1d2dce38f86a13b1e31e78f"),
    "selected/contracts/COMBINED.md": ("sdk/consumer-setup/COMBINED.md", "bba61b148c12ebdd8e5730bdaf997edc726431e5118bc509883b32dc438f9a7b"),
    "selected/contracts/SETUP-SELECTED.md": ("sdk/consumer-setup/SELECTED.md", "ef788c8e42916c18d68eb234b1a0e8867be204f25e60a6a924de62b3377fb7ed"),
    "selected/contracts/PYTHON-ACQUISITION.md": ("sdk/consumer-setup/PYTHON-ACQUISITION.md", "0dce034f8acb66e71962fc5b4a98790018046a07740446f87a3e0f902345498c"),
    "selected/contracts/ACQUISITION-RECOVERY.md": ("sdk/consumer-setup/ACQUISITION-RECOVERY.md", "fb266b7a7201686686a629ce38c93229d2d4407c11b713e4cdda1c77d7d8e668"),
    "selected/contracts/MULTI-RECOVERY.md": ("sdk/consumer-setup/MULTI-RECOVERY.md", "fe82e1ec5a9334b26d76c6c05983b48f9d332dad1a385981fa6e2b13377eaf0a"),
    "selected/contracts/METADATA.md": ("sdk/consumer-setup/METADATA.md", "034faa69b582d625e99d59cd043eaf9400c30a3bc526b836bdb856a8710337fc"),
}
PYTHON_DONOR_CONTRACTS = {
    "selected/contracts/NODE-INSTALL.md": "crates/toolkit-package/NODE-INSTALL.md",
    "selected/contracts/PYTHON-INSTALL.md": "crates/toolkit-package/PYTHON-INSTALL.md",
    "selected/contracts/INSPECT.md": "crates/toolkit-package/INSPECT.md",
}
SOURCE_REVISION = "ebe4e63ea080f92c99226c80d18d03fd28e1e0ae"


# Native connector consumer successor; setup and installer owners stay pinned.
TARGETS['macos-arm64'].update({'edition': '20261009-library-support-selection',
 'guide': 'docs/verification/20261009-library-support-selection/macos-arm64-release.md',
 'guide_sha256': 'aa10e10b72aebc5292b05d161110854289b403555ab611fb3a73b6099433beef'})
PYTHON_KITS['macos-arm64']["tool_updates"]["bin/toolkit-package-inspect"] = {'record': 'docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/toolkit-package-inspect-build.json',
 'sha256': '8a19b3daa6914069e78d8c3bce549ba2bd87d93b448c6726caf7239909042d78',
 'source': 'docs/verification/20261008-mcpb-canonical-delivery/mac/toolkit-package-inspect'}
PYTHON_KITS['macos-arm64']["sources"].update({'selected/provenance/canonical-mcpb/Cargo.lock': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/Cargo.lock',
                                                   'f911a612fd54bd1634e9a1d36a955198723779c2fcd43efded9acb82885f357a'),
 'selected/provenance/canonical-mcpb/crates/adler2-2.0.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/adler2-2.0.1.crate',
                                                                  '320119579fcad9c21884f5c4861d16174d0e06250625266f50fe6898340abefa'),
 'selected/provenance/canonical-mcpb/crates/ambient-authority-0.0.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/ambient-authority-0.0.2.crate',
                                                                             'e9d4ee0d472d1cd2e28c97dfa124b3d8d992e10eb0a035f33f5d12e3a177ba3b'),
 'selected/provenance/canonical-mcpb/crates/bitflags-2.13.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/bitflags-2.13.2.crate',
                                                                     '3ded4057c258ba199e2d26386d3af3780957ecaee6c4ef4041c6b4b8b97c0b06'),
 'selected/provenance/canonical-mcpb/crates/block-buffer-0.10.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/block-buffer-0.10.4.crate',
                                                                         '3078c7629b62d3f0439517fa394996acacc5cbc91c5a20d8c658e77abd503a71'),
 'selected/provenance/canonical-mcpb/crates/cap-primitives-4.0.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cap-primitives-4.0.3.crate',
                                                                          '8b5f74729fd2f44701d1a8eb47e906cdb3ccd9ec0f02baad85a744b791940b18'),
 'selected/provenance/canonical-mcpb/crates/cap-std-4.0.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cap-std-4.0.3.crate',
                                                                   'c1ec78e242cfa2cfe276807ac2ecc00315a6c97786977414bcd1c3963b6c91b8'),
 'selected/provenance/canonical-mcpb/crates/cfg-if-1.0.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cfg-if-1.0.5.crate',
                                                                  '4e7648175b45a9a48536d676f68d918270699102aa8dab5496df06904c914600'),
 'selected/provenance/canonical-mcpb/crates/cpufeatures-0.2.17.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cpufeatures-0.2.17.crate',
                                                                        '59ed5838eebb26a2bb2e58f6d5b5316989ae9d08bab10e0e6d103e656d1b0280'),
 'selected/provenance/canonical-mcpb/crates/crc32fast-1.5.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/crc32fast-1.5.2.crate',
                                                                     '01a7799fd6b852db0e61728dde9a204c423b44d689dbd432522543614b490e78'),
 'selected/provenance/canonical-mcpb/crates/crypto-common-0.1.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/crypto-common-0.1.7.crate',
                                                                         '78c8292055d1c1df0cce5d180393dc8cce0abec0a7102adb6c7b1eef6016d60a'),
 'selected/provenance/canonical-mcpb/crates/digest-0.10.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/digest-0.10.7.crate',
                                                                   '9ed9a281f7bc9b7576e61468ba615a66a5c8cfdff42420a70aa82701a3b1e292'),
 'selected/provenance/canonical-mcpb/crates/displaydoc-0.2.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/displaydoc-0.2.7.crate',
                                                                      'c6232dd377dcc64799954cbd3a9bb882e9cdc1308ccd87b1c098f1fb2eaf82a8'),
 'selected/provenance/canonical-mcpb/crates/equivalent-1.0.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/equivalent-1.0.2.crate',
                                                                      '877a4ace8713b0bcf2a4e7eec82529c029f1d0619886d18145fea96c3ffe5c0f'),
 'selected/provenance/canonical-mcpb/crates/errno-0.3.14.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/errno-0.3.14.crate',
                                                                  '39cab71617ae0d63f51a36d69f866391735b51691dbda63cf6f96d042b63efeb'),
 'selected/provenance/canonical-mcpb/crates/flate2-1.1.10.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/flate2-1.1.10.crate',
                                                                   '6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb'),
 'selected/provenance/canonical-mcpb/crates/form_urlencoded-1.2.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/form_urlencoded-1.2.2.crate',
                                                                           'cb4cb245038516f5f85277875cdaa4f7d2c9a0fa0468de06ed190163b1581fcf'),
 'selected/provenance/canonical-mcpb/crates/fs-set-times-0.20.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/fs-set-times-0.20.3.crate',
                                                                         '94e7099f6313ecacbe1256e8ff9d617b75d1bcb16a6fddef94866d225a01a14a'),
 'selected/provenance/canonical-mcpb/crates/generic-array-0.14.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/generic-array-0.14.7.crate',
                                                                          '85649ca51fd72272d7821adaf274ad91c288277713d9c18820d8499a7ff69e9a'),
 'selected/provenance/canonical-mcpb/crates/hashbrown-0.17.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/hashbrown-0.17.1.crate',
                                                                      'ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a'),
 'selected/provenance/canonical-mcpb/crates/hex-0.4.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/hex-0.4.3.crate',
                                                               '7f24254aa9a54b5c858eaee2f5bccdb46aaf0e486a595ed5fd8f86ba55232a70'),
 'selected/provenance/canonical-mcpb/crates/icu_collections-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_collections-2.3.0.crate',
                                                                           'fa68d21081c4a05d5a901a1c62add574c77048b6a1c67be3b50ce0b60d4ca513'),
 'selected/provenance/canonical-mcpb/crates/icu_locale_core-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_locale_core-2.3.0.crate',
                                                                           'd56e28588da92eee5c3201a6eff33fabdd49b62269c8938d4ff050ce4d900deb'),
 'selected/provenance/canonical-mcpb/crates/icu_normalizer-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_normalizer-2.3.0.crate',
                                                                          '12f9cf5f235641ed274641dd81c3f28d870e276763d0797aeeab72317b1c646f'),
 'selected/provenance/canonical-mcpb/crates/icu_normalizer_data-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_normalizer_data-2.3.0.crate',
                                                                               '1563da1ed3e0b3bf3d74c9b85917ac9c56464d2f57242270c09c9e752f8021a0'),
 'selected/provenance/canonical-mcpb/crates/icu_properties-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_properties-2.3.0.crate',
                                                                          '7e7ca276ad3145661a65914e6daf131ca5120cd3dcee8f8f3214b8875184a148'),
 'selected/provenance/canonical-mcpb/crates/icu_properties_data-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_properties_data-2.3.0.crate',
                                                                               'e590f038c1464a96894fd6d10127e90a8be4509f56ff7ecef851b15cee0b7caa'),
 'selected/provenance/canonical-mcpb/crates/icu_provider-2.3.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_provider-2.3.1.crate',
                                                                        'd27bbb9d3abbefac45d55f647c9de1d44aafcd1186eb91879afef17c396c3e73'),
 'selected/provenance/canonical-mcpb/crates/idna-1.1.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/idna-1.1.0.crate',
                                                                '3b0875f23caa03898994f6ddc501886a45c7d3d62d04d2d90788d47be1b1e4de'),
 'selected/provenance/canonical-mcpb/crates/idna_adapter-1.2.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/idna_adapter-1.2.2.crate',
                                                                        'cb68373c0d6620ef8105e855e7745e18b0d00d3bdb07fb532e434244cdb9a714'),
 'selected/provenance/canonical-mcpb/crates/indexmap-2.14.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/indexmap-2.14.2.crate',
                                                                     'cc4e190f5d26ca7051642629da2c52fc03bde85a03197c99408dcd291734c855'),
 'selected/provenance/canonical-mcpb/crates/io-extras-0.19.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/io-extras-0.19.0.crate',
                                                                      '20fd6de4ccfcc187e38bc21cfa543cb5a302cb86a8b114eb7f0bf0dc9f8ac00f'),
 'selected/provenance/canonical-mcpb/crates/io-lifetimes-2.0.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/io-lifetimes-2.0.4.crate',
                                                                        '06432fb54d3be7964ecd3649233cddf80db2832f47fec34c01f65b3d9d774983'),
 'selected/provenance/canonical-mcpb/crates/io-lifetimes-3.0.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/io-lifetimes-3.0.1.crate',
                                                                        '2f0fb0570afe1fed943c5c3d4102d5358592d8625fda6a0007fdbe65a92fba96'),
 'selected/provenance/canonical-mcpb/crates/ipnet-2.12.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/ipnet-2.12.2.crate',
                                                                  '791930b43c0d5973160d90a8f3894509f2b273430f5c5c73b668636d0287c5c0'),
 'selected/provenance/canonical-mcpb/crates/itoa-1.0.18.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/itoa-1.0.18.crate',
                                                                 '8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682'),
 'selected/provenance/canonical-mcpb/crates/libc-0.2.189.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/libc-0.2.189.crate',
                                                                  '3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2'),
 'selected/provenance/canonical-mcpb/crates/linux-raw-sys-0.12.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/linux-raw-sys-0.12.1.crate',
                                                                          '32a66949e030da00e8c7d4434b251670a91556f4144941d37452769c25d58a53'),
 'selected/provenance/canonical-mcpb/crates/litemap-0.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/litemap-0.8.3.crate',
                                                                   '47d9d19d1d6efa0109d2f65ff4c85cddd50bd572e5a00127ab10987290bcefae'),
 'selected/provenance/canonical-mcpb/crates/maybe-owned-0.3.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/maybe-owned-0.3.4.crate',
                                                                       '4facc753ae494aeb6e3c22f839b158aebd4f9270f55cd3c79906c45476c47ab4'),
 'selected/provenance/canonical-mcpb/crates/memchr-2.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/memchr-2.8.3.crate',
                                                                  'cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98'),
 'selected/provenance/canonical-mcpb/crates/miniz_oxide-0.9.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/miniz_oxide-0.9.1.crate',
                                                                       'b63fbc4a50860e98e7b2aa7804ded1db5cbc3aff9193adaff57a6931bf7c4b4c'),
 'selected/provenance/canonical-mcpb/crates/once_cell-1.21.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/once_cell-1.21.4.crate',
                                                                      '9f7c3e4beb33f85d45ae3e3a1792185706c8e16d043238c593331cc7cd313b50'),
 'selected/provenance/canonical-mcpb/crates/percent-encoding-2.3.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/percent-encoding-2.3.2.crate',
                                                                            '9b4f627cb1b25917193a259e49bdad08f671f8d9708acfd5fe0a8c1455d87220'),
 'selected/provenance/canonical-mcpb/crates/potential_utf-0.1.6.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/potential_utf-0.1.6.crate',
                                                                         'd83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661'),
 'selected/provenance/canonical-mcpb/crates/proc-macro2-1.0.107.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/proc-macro2-1.0.107.crate',
                                                                         '985e7ec9bb745e6ce6535b544d84d6cd6f7ad8bd711c398938ae983b91a766d9'),
 'selected/provenance/canonical-mcpb/crates/quote-1.0.47.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/quote-1.0.47.crate',
                                                                  '1fbf4db142a473a8d80c26bbf18454ed458bf8d26c8219c331daecfdbd079001'),
 'selected/provenance/canonical-mcpb/crates/rustix-1.1.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/rustix-1.1.5.crate',
                                                                  '891efababe418670775f199f0d233d84843c227a0949a883ce15b37c78d6629d'),
 'selected/provenance/canonical-mcpb/crates/rustix-linux-procfs-0.1.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/rustix-linux-procfs-0.1.1.crate',
                                                                               '2fc84bf7e9aa16c4f2c758f27412dc9841341e16aa682d9c7ac308fe3ee12056'),
 'selected/provenance/canonical-mcpb/crates/serde-1.0.229.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde-1.0.229.crate',
                                                                   '4148590afebada386688f18773da617792bf2ef03ffc1e4cbd2b1d45b023e0ba'),
 'selected/provenance/canonical-mcpb/crates/serde_core-1.0.229.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_core-1.0.229.crate',
                                                                        '67dca2c9c51e58a4791a4b1ed58308b39c64224d349a935ab5039aa360942a48'),
 'selected/provenance/canonical-mcpb/crates/serde_derive-1.0.229.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_derive-1.0.229.crate',
                                                                          'e7a5d71263a5a7d47b41f6b3f06ba276f10cc18b0931f1799f710578e2309348'),
 'selected/provenance/canonical-mcpb/crates/serde_json-1.0.151.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_json-1.0.151.crate',
                                                                        'c841b55ecdae098c80dcae9cf767f6f8a0c2cdb3416bbef72181df4d0fe73f14'),
 'selected/provenance/canonical-mcpb/crates/serde_path_to_error-0.1.20.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_path_to_error-0.1.20.crate',
                                                                                '10a9ff822e371bb5403e391ecd83e182e0e77ba7f6fe0160b795797109d1b457'),
 'selected/provenance/canonical-mcpb/crates/sha2-0.10.9.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/sha2-0.10.9.crate',
                                                                 'a7507d819769d01a365ab707794a4084392c824f54a7a6a7862f8c3d0892b283'),
 'selected/provenance/canonical-mcpb/crates/simd-adler32-0.3.10.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/simd-adler32-0.3.10.crate',
                                                                         '3a219298ac11a56ea9a6d2120044824d6f01aeb034955e7af7bc16858527deea'),
 'selected/provenance/canonical-mcpb/crates/smallvec-1.16.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/smallvec-1.16.2.crate',
                                                                     'f9395f0f0eee849a9b707b2f06bb92a6a422090e2123bb2ef8e87a0e61892a8e'),
 'selected/provenance/canonical-mcpb/crates/stable_deref_trait-1.2.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/stable_deref_trait-1.2.1.crate',
                                                                              '6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596'),
 'selected/provenance/canonical-mcpb/crates/syn-3.0.6.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/syn-3.0.6.crate',
                                                               '8593e8e72159ed2257d083c7a454a85cbf854f37a0966d8d483aff8c8a3ebcee'),
 'selected/provenance/canonical-mcpb/crates/synstructure-0.14.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/synstructure-0.14.0.crate',
                                                                         '901704edd0dfe137f1987838ee4f259e4e063c31371bdb423f7ae38ec6f77f02'),
 'selected/provenance/canonical-mcpb/crates/thiserror-2.0.21.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/thiserror-2.0.21.crate',
                                                                      '09e52cb86a36cede5cb101bf8908837b3e4c6e5e59fe7fd85c23fb56200d189e'),
 'selected/provenance/canonical-mcpb/crates/thiserror-impl-2.0.21.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/thiserror-impl-2.0.21.crate',
                                                                           'fe5197923287db20a58125f0bc85c062f7f2c892de97b18c356f9efb14b28524'),
 'selected/provenance/canonical-mcpb/crates/tinystr-0.8.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/tinystr-0.8.4.crate',
                                                                   'b1e27c91459209c2986af3dcf603a5a74a4368754ce37414f59acc971167f643'),
 'selected/provenance/canonical-mcpb/crates/typenum-1.20.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/typenum-1.20.1.crate',
                                                                    'b6f5e870be6c3b371b77fe0ee0bafb859fa4964b4404c27de1d380043c4dda20'),
 'selected/provenance/canonical-mcpb/crates/unicode-ident-1.0.26.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/unicode-ident-1.0.26.crate',
                                                                          'd245f478577f809a851594d02313b640fb437e0bb33866753cff937863096954'),
 'selected/provenance/canonical-mcpb/crates/url-2.5.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/url-2.5.8.crate',
                                                               'ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed'),
 'selected/provenance/canonical-mcpb/crates/utf8_iter-1.0.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/utf8_iter-1.0.4.crate',
                                                                     'b6c140620e7ffbb22c2dee59cafe6084a59b5ffc27a8859a5f0d494b5d52b6be'),
 'selected/provenance/canonical-mcpb/crates/version_check-0.9.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/version_check-0.9.5.crate',
                                                                         '0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a'),
 'selected/provenance/canonical-mcpb/crates/writeable-0.6.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/writeable-0.6.4.crate',
                                                                     '3ad82d2a33cdc9674dc7465672f271e096168fcdbe0f799d9e6db8c5892679dc'),
 'selected/provenance/canonical-mcpb/crates/yoke-0.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/yoke-0.8.3.crate',
                                                                '709fe23a0424b6a435d82152b1bd3fdfb0833487d5fa90d05d42762a9891fef5'),
 'selected/provenance/canonical-mcpb/crates/yoke-derive-0.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/yoke-derive-0.8.3.crate',
                                                                       '33811428bee40dbceb6d545e95754741d17a6aef9a4849f0fd62e2ba4f412a78'),
 'selected/provenance/canonical-mcpb/crates/zerofrom-0.1.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerofrom-0.1.8.crate',
                                                                    '0ec05a11813ea801ff6d75110ad09cd0824ddba17dfe17128ea0d5f68e6c5272'),
 'selected/provenance/canonical-mcpb/crates/zerofrom-derive-0.1.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerofrom-derive-0.1.8.crate',
                                                                           'f75b4683f6c7f45248d4d64056a24298c6281e0993356d7d1b4a1a962ef10d4a'),
 'selected/provenance/canonical-mcpb/crates/zerotrie-0.2.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerotrie-0.2.5.crate',
                                                                    '4ea269c3bd32f0a32c321907a2ae912ba6f4649bb0fc764a15627e99a7095a3f'),
 'selected/provenance/canonical-mcpb/crates/zerovec-0.11.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerovec-0.11.8.crate',
                                                                    'bb0464e17806c1d976d5cba29399c7f08e516e279e2ba493f63123b5fca67dd8'),
 'selected/provenance/canonical-mcpb/crates/zerovec-derive-0.11.6.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerovec-derive-0.11.6.crate',
                                                                           '34df6fc39dbd26ddc9c10e6a2984476e13acce22e64e4487636ef494369225da'),
 'selected/provenance/canonical-mcpb/crates/zip-2.4.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zip-2.4.2.crate',
                                                               'fabe6324e908f85a1c52063ce7aa26b68dcb7eb6dbc83a2d148403c9bc3eba50'),
 'selected/provenance/canonical-mcpb/crates/zmij-1.0.23.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zmij-1.0.23.crate',
                                                                 '29666d0abbfad1e3dc4dcf6144730dd3a3ab225bbbdac83319345b1b44ccfc1b'),
 'selected/provenance/canonical-mcpb/licenses/adler2-2.0.1/LICENSE-0BSD': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/adler2-2.0.1/LICENSE-0BSD',
                                                                           '861399f8c21c042b110517e76dc6b63a2b334276c8cf17412fc3c8908ca8dc17'),
 'selected/provenance/canonical-mcpb/licenses/adler2-2.0.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/adler2-2.0.1/LICENSE-APACHE',
                                                                             '8ada45cd9f843acf64e4722ae262c622a2b3b3007c7310ef36ac1061a30f6adb'),
 'selected/provenance/canonical-mcpb/licenses/adler2-2.0.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/adler2-2.0.1/LICENSE-MIT',
                                                                          '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/COPYRIGHT',
                                                                                   'd00d6d48df2db6f91e4ca9c656e5e30782d5becf52bb3e5bfd0aff467bd37781'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/LICENSE-APACHE',
                                                                                        'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/LICENSE-MIT',
                                                                                     '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/bitflags-2.13.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/bitflags-2.13.2/LICENSE-APACHE',
                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/bitflags-2.13.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/bitflags-2.13.2/LICENSE-MIT',
                                                                             '6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb'),
 'selected/provenance/canonical-mcpb/licenses/block-buffer-0.10.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/block-buffer-0.10.4/LICENSE-APACHE',
                                                                                    'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/block-buffer-0.10.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/block-buffer-0.10.4/LICENSE-MIT',
                                                                                 'd5c22aa3118d240e877ad41c5d9fa232f9c77d757d4aac0c2f943afc0a95e0ef'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/COPYRIGHT',
                                                                                'a8799c2a07dabcb8d52481f885c5ab7000acb8a4d3cd9503fbcb97c84d350e65'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/LICENSE-APACHE',
                                                                                     'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                             '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/COPYRIGHT',
                                                                         '6b1691fdee3b03c3946696a7be125fcf50eff310b9486fac850c1b8aea91478b'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/LICENSE-APACHE',
                                                                              'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                      '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/LICENSE-MIT',
                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/cfg-if-1.0.5/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cfg-if-1.0.5/LICENSE-APACHE',
                                                                             'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/cfg-if-1.0.5/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cfg-if-1.0.5/LICENSE-MIT',
                                                                          '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
 'selected/provenance/canonical-mcpb/licenses/cpufeatures-0.2.17/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cpufeatures-0.2.17/LICENSE-APACHE',
                                                                                   'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/cpufeatures-0.2.17/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cpufeatures-0.2.17/LICENSE-MIT',
                                                                                'ae9baa7beea910273c2f384c2a6b721fb7bd02bda3436074a1072e4ee689f985'),
 'selected/provenance/canonical-mcpb/licenses/crc32fast-1.5.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crc32fast-1.5.2/LICENSE-APACHE',
                                                                                'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
 'selected/provenance/canonical-mcpb/licenses/crc32fast-1.5.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crc32fast-1.5.2/LICENSE-MIT',
                                                                             '61d383b05b87d78f94d2937e2580cce47226d17823c0430fbcad09596537efcf'),
 'selected/provenance/canonical-mcpb/licenses/crypto-common-0.1.7/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crypto-common-0.1.7/LICENSE-APACHE',
                                                                                    'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/crypto-common-0.1.7/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crypto-common-0.1.7/LICENSE-MIT',
                                                                                 '3521672491a3479422d5fe1aca6645dd2984090f85da6e5205abfb18fb7a6897'),
 'selected/provenance/canonical-mcpb/licenses/digest-0.10.7/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/digest-0.10.7/LICENSE-APACHE',
                                                                              'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/digest-0.10.7/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/digest-0.10.7/LICENSE-MIT',
                                                                           '9e0dfd2dd4173a530e238cb6adb37aa78c34c6bc7444e0e10c1ab5d8881f63ba'),
 'selected/provenance/canonical-mcpb/licenses/displaydoc-0.2.7/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/displaydoc-0.2.7/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/displaydoc-0.2.7/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/displaydoc-0.2.7/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/equivalent-1.0.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/equivalent-1.0.2/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/equivalent-1.0.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/equivalent-1.0.2/LICENSE-MIT',
                                                                              '7365cc8878a1d7ce155a58c4ca09c3d7a6be413efa5334a80ea842912b669349'),
 'selected/provenance/canonical-mcpb/licenses/errno-0.3.14/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/errno-0.3.14/LICENSE-APACHE',
                                                                             'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/errno-0.3.14/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/errno-0.3.14/LICENSE-MIT',
                                                                          '8764a597675778ddfd4e25f81b08a05dbcf089ac05662df7613fe67f150e3aa2'),
 'selected/provenance/canonical-mcpb/licenses/flate2-1.1.10/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/flate2-1.1.10/LICENSE-APACHE',
                                                                              'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/flate2-1.1.10/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/flate2-1.1.10/LICENSE-MIT',
                                                                           '025436edff4cfcdde17a5811fdea78892d8482efd1abdec5a17872d07a4f2112'),
 'selected/provenance/canonical-mcpb/licenses/form_urlencoded-1.2.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/form_urlencoded-1.2.2/LICENSE-APACHE',
                                                                                      'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/form_urlencoded-1.2.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/form_urlencoded-1.2.2/LICENSE-MIT',
                                                                                   '20c7855c364d57ea4c97889a5e8d98470a9952dade37bd9248b9a54431670e5e'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/COPYRIGHT',
                                                                               'e196ffeb5ab101e9ec8bbe395c9f367c5380ee2d28c47e5c03371523798a9af7'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/LICENSE-APACHE',
                                                                                    'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                            '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/LICENSE-MIT',
                                                                                 '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/generic-array-0.14.7/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/generic-array-0.14.7/LICENSE',
                                                                              'c09aae9d3c77b531f56351a9947bc7446511d6b025b3255312d3e3442a9a7583'),
 'selected/provenance/canonical-mcpb/licenses/hashbrown-0.17.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hashbrown-0.17.1/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/hashbrown-0.17.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hashbrown-0.17.1/LICENSE-MIT',
                                                                              'ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2'),
 'selected/provenance/canonical-mcpb/licenses/hex-0.4.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hex-0.4.3/LICENSE-APACHE',
                                                                          'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
 'selected/provenance/canonical-mcpb/licenses/hex-0.4.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hex-0.4.3/LICENSE-MIT',
                                                                       'f7bdb3426d045cd50efd4953026e3eb5a83d0199f458a075602611b9344da5b9'),
 'selected/provenance/canonical-mcpb/licenses/icu_collections-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_collections-2.3.0/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_locale_core-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_locale_core-2.3.0/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_normalizer-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_normalizer-2.3.0/LICENSE',
                                                                              'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_normalizer_data-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_normalizer_data-2.3.0/LICENSE',
                                                                                   'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_properties-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_properties-2.3.0/LICENSE',
                                                                              'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_properties_data-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_properties_data-2.3.0/LICENSE',
                                                                                   'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_provider-2.3.1/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_provider-2.3.1/LICENSE',
                                                                            'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/idna-1.1.0/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna-1.1.0/LICENSE-APACHE',
                                                                           'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/idna-1.1.0/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna-1.1.0/LICENSE-MIT',
                                                                        'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
 'selected/provenance/canonical-mcpb/licenses/idna_adapter-1.2.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna_adapter-1.2.2/LICENSE-APACHE',
                                                                                   'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/idna_adapter-1.2.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna_adapter-1.2.2/LICENSE-MIT',
                                                                                '8b43ce8accd61e9d370b5ca9e9c4f953279b5c239926c62315b40e24df51b726'),
 'selected/provenance/canonical-mcpb/licenses/indexmap-2.14.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/indexmap-2.14.2/LICENSE-APACHE',
                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/indexmap-2.14.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/indexmap-2.14.2/LICENSE-MIT',
                                                                             'ecc269ef87fd38a1d98e30bfac9ba964a9dbd9315c3770fed98d4d7cb5882055'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/COPYRIGHT',
                                                                            'f0c1d22de8bd3b776a40f2691f08b84e91c7bafc78b72a51150e231002b88db3'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                         '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/COPYRIGHT',
                                                                              '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/LICENSE-APACHE',
                                                                                   'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                           '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/COPYRIGHT',
                                                                              '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/LICENSE-APACHE',
                                                                                   'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                           '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/ipnet-2.12.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ipnet-2.12.2/LICENSE-APACHE',
                                                                             '87d9feb9238c6bd8e0024fc4733b06cff036f89f36d93b7df1c8a0549bbb7a5b'),
 'selected/provenance/canonical-mcpb/licenses/ipnet-2.12.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ipnet-2.12.2/LICENSE-MIT',
                                                                          '47dc9ff29128ddfb4d6a0435383c9f89120bc374dbcc1dd00b933a0b28aa7865'),
 'selected/provenance/canonical-mcpb/licenses/itoa-1.0.18/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/itoa-1.0.18/LICENSE-APACHE',
                                                                            '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/itoa-1.0.18/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/itoa-1.0.18/LICENSE-MIT',
                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/libc-0.2.189/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/libc-0.2.189/LICENSE-APACHE',
                                                                             '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/libc-0.2.189/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/libc-0.2.189/LICENSE-MIT',
                                                                          '123a331b5dbf04c30097fa43b8f858bc85df671fe776de498d01f3d6b7c1f69e'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/COPYRIGHT',
                                                                                '3290ae0fbc9ddb77d2239121d710f0bb9d31b3b4744e6d97fe01e652b4c1870b'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-APACHE',
                                                                                     'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                             '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/litemap-0.8.3/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/litemap-0.8.3/LICENSE',
                                                                       'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/maybe-owned-0.3.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/maybe-owned-0.3.4/LICENSE-APACHE',
                                                                                  'aecc100d0547aa3bcbe04bcf5cb87c7db0be11004602adfa5056edf8bb847b35'),
 'selected/provenance/canonical-mcpb/licenses/maybe-owned-0.3.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/maybe-owned-0.3.4/LICENSE-MIT',
                                                                               'ef2b3bbd7b718a7881253226313f103e89b379e1279681a3c1ebcb65a27d209d'),
 'selected/provenance/canonical-mcpb/licenses/memchr-2.8.3/COPYING': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/memchr-2.8.3/COPYING',
                                                                      '01c266bced4a434da0051174d6bee16a4c82cf634e2679b6155d40d75012390f'),
 'selected/provenance/canonical-mcpb/licenses/memchr-2.8.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/memchr-2.8.3/LICENSE-MIT',
                                                                          '0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE',
                                                                           '4108245a1f2df9d4e94df8abed5b4ba0759bb2f9b40a6b939f1be141077ae50b'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE-APACHE.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE-APACHE.md',
                                                                                     '0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE-MIT.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE-MIT.md',
                                                                                  '799e9ca9d179295ef372f25d3769cdda7d25bb2668add6a6a1e22d1e4c678b8d'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE-ZLIB.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE-ZLIB.md',
                                                                                   '0a54e647fe54104658b5e563c04c6f9edf251710e47bce692e0bd990a4ddaa39'),
 'selected/provenance/canonical-mcpb/licenses/once_cell-1.21.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/once_cell-1.21.4/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/once_cell-1.21.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/once_cell-1.21.4/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/percent-encoding-2.3.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/percent-encoding-2.3.2/LICENSE-APACHE',
                                                                                       'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/percent-encoding-2.3.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/percent-encoding-2.3.2/LICENSE-MIT',
                                                                                    'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
 'selected/provenance/canonical-mcpb/licenses/potential_utf-0.1.6/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/potential_utf-0.1.6/LICENSE',
                                                                             'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/proc-macro2-1.0.107/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/proc-macro2-1.0.107/LICENSE-APACHE',
                                                                                    '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/proc-macro2-1.0.107/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/proc-macro2-1.0.107/LICENSE-MIT',
                                                                                 '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/quote-1.0.47/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/quote-1.0.47/LICENSE-APACHE',
                                                                             '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/quote-1.0.47/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/quote-1.0.47/LICENSE-MIT',
                                                                          '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/COPYRIGHT',
                                                                        '377c2e7c53250cc5905c0b0532d35973392af16ffb9596a41d99d202cf3617c9'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/LICENSE-APACHE',
                                                                             'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                     '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/LICENSE-MIT',
                                                                          '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/COPYRIGHT',
                                                                                     'd415a86ccfd794129666247cbbadebee7a6d0ac3ff701c76c07b0f9424f0307d'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-APACHE',
                                                                                          'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                  '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-MIT',
                                                                                       '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde-1.0.229/LICENSE-APACHE',
                                                                              '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde-1.0.229/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde-1.0.229/LICENSE-MIT',
                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_core-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_core-1.0.229/LICENSE-APACHE',
                                                                                   '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_core-1.0.229/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_core-1.0.229/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_derive-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_derive-1.0.229/LICENSE-APACHE',
                                                                                     '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_derive-1.0.229/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_derive-1.0.229/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_json-1.0.151/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_json-1.0.151/LICENSE-APACHE',
                                                                                   '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_json-1.0.151/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_json-1.0.151/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_path_to_error-0.1.20/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_path_to_error-0.1.20/LICENSE-APACHE',
                                                                                           '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_path_to_error-0.1.20/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_path_to_error-0.1.20/LICENSE-MIT',
                                                                                        '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/sha2-0.10.9/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/sha2-0.10.9/LICENSE-APACHE',
                                                                            'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/sha2-0.10.9/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/sha2-0.10.9/LICENSE-MIT',
                                                                         'b4eb00df6e2a4d22518fcaa6a2b4646f249b3a3c9814509b22bd2091f1392ff1'),
 'selected/provenance/canonical-mcpb/licenses/simd-adler32-0.3.10/LICENSE.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/simd-adler32-0.3.10/LICENSE.md',
                                                                                '42a35170233e83e18856792e748de4c1ce4a63b2afce9a370c89ef3fe23f9f2d'),
 'selected/provenance/canonical-mcpb/licenses/smallvec-1.16.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/smallvec-1.16.2/LICENSE-APACHE',
                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/smallvec-1.16.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/smallvec-1.16.2/LICENSE-MIT',
                                                                             '0b28172679e0009b655da42797c03fd163a3379d5cfa67ba1f1655e974a2a1a9'),
 'selected/provenance/canonical-mcpb/licenses/stable_deref_trait-1.2.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/stable_deref_trait-1.2.1/LICENSE-APACHE',
                                                                                         'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/stable_deref_trait-1.2.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/stable_deref_trait-1.2.1/LICENSE-MIT',
                                                                                      '5e05b024f653a5ce199e77cbbbd42fb5553562ec714b819421ed0c3e552a75d7'),
 'selected/provenance/canonical-mcpb/licenses/syn-3.0.6/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/syn-3.0.6/LICENSE-APACHE',
                                                                          '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/syn-3.0.6/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/syn-3.0.6/LICENSE-MIT',
                                                                       '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/synstructure-0.14.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/synstructure-0.14.0/LICENSE',
                                                                             '219920e865eee70b7dcfc948a86b099e7f4fe2de01bcca2ca9a20c0a033f2b59'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-2.0.21/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-2.0.21/LICENSE-APACHE',
                                                                                 '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-2.0.21/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-2.0.21/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-impl-2.0.21/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-impl-2.0.21/LICENSE-APACHE',
                                                                                      '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-impl-2.0.21/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-impl-2.0.21/LICENSE-MIT',
                                                                                   '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/tinystr-0.8.4/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/tinystr-0.8.4/LICENSE',
                                                                       'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/typenum-1.20.1/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/typenum-1.20.1/LICENSE',
                                                                        'db11fec9946737df39ca3898d9cd8c10ec6f6c3a884a6802b0ad0b81b4e8f23a'),
 'selected/provenance/canonical-mcpb/licenses/typenum-1.20.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/typenum-1.20.1/LICENSE-APACHE',
                                                                               '516b24e051bf5630880ebbd55c40a25ce9552ebaf8970a53e8976eb70e522406'),
 'selected/provenance/canonical-mcpb/licenses/typenum-1.20.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/typenum-1.20.1/LICENSE-MIT',
                                                                            'a825bd853ab71619a4923d7b4311221427848070ff44d990da39b0b274c1683f'),
 'selected/provenance/canonical-mcpb/licenses/unicode-ident-1.0.26/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/unicode-ident-1.0.26/LICENSE-APACHE',
                                                                                     '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/unicode-ident-1.0.26/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/unicode-ident-1.0.26/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/unicode-ident-1.0.26/LICENSE-UNICODE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/unicode-ident-1.0.26/LICENSE-UNICODE',
                                                                                      'f7db81051789b729fea528a63ec4c938fdcb93d9d61d97dc8cc2e9df6d47f2a1'),
 'selected/provenance/canonical-mcpb/licenses/url-2.5.8/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/url-2.5.8/LICENSE-APACHE',
                                                                          'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/url-2.5.8/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/url-2.5.8/LICENSE-MIT',
                                                                       'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
 'selected/provenance/canonical-mcpb/licenses/utf8_iter-1.0.4/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/utf8_iter-1.0.4/COPYRIGHT',
                                                                           'c30152c94a6d75e021adbc52b3a52470366a46edb917e17deae3259251af244c'),
 'selected/provenance/canonical-mcpb/licenses/utf8_iter-1.0.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/utf8_iter-1.0.4/LICENSE-APACHE',
                                                                                'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'),
 'selected/provenance/canonical-mcpb/licenses/utf8_iter-1.0.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/utf8_iter-1.0.4/LICENSE-MIT',
                                                                             '3fa4ca83dcc9237839b1bdeb2e6d16bdfb5ec0c5ce42b24694d8bbf0dcbef72c'),
 'selected/provenance/canonical-mcpb/licenses/version_check-0.9.5/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/version_check-0.9.5/LICENSE-APACHE',
                                                                                    'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/version_check-0.9.5/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/version_check-0.9.5/LICENSE-MIT',
                                                                                 'b7e650f3fce5c53249d1cdc608b54df156a97edd636cf9d23498d0cfe7aec63e'),
 'selected/provenance/canonical-mcpb/licenses/writeable-0.6.4/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/writeable-0.6.4/LICENSE',
                                                                         'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/yoke-0.8.3/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/yoke-0.8.3/LICENSE',
                                                                    'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/yoke-derive-0.8.3/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/yoke-derive-0.8.3/LICENSE',
                                                                           'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerofrom-0.1.8/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerofrom-0.1.8/LICENSE',
                                                                        'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerofrom-derive-0.1.8/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerofrom-derive-0.1.8/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerotrie-0.2.5/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerotrie-0.2.5/LICENSE',
                                                                        'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerovec-0.11.8/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerovec-0.11.8/LICENSE',
                                                                        'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerovec-derive-0.11.6/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerovec-derive-0.11.6/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zip-2.4.2/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zip-2.4.2/LICENSE',
                                                                   '04dc52136d82400b11df5f474949863a70a5d05d2278dec45b99c479494450a6'),
 'selected/provenance/canonical-mcpb/licenses/zmij-1.0.23/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zmij-1.0.23/LICENSE-MIT',
                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/macos-arm64/build-result.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/build-result.json',
                                                                      '8f3e05f16123c032d030bcff3eb3095ad5d2dea8e5e7870750d76eabed7c8448'),
 'selected/provenance/canonical-mcpb/macos-arm64/build.stdout': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/build.stdout',
                                                                 '5e249257b239cbc124fb938ff8b47d325905ea074f73a9e608fc30e04000b3f0'),
 'selected/provenance/canonical-mcpb/macos-arm64/commands.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/commands.json',
                                                                  'e86d18129e023475e27e013d527b72b42c927b086a1af27c5aa721e3b485b956'),
 'selected/provenance/canonical-mcpb/macos-arm64/dependencies.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/dependencies.json',
                                                                      '3dae349e5749f0a4720120f34479975eb50b890132a82c609b7443d6f42fb38f'),
 'selected/provenance/canonical-mcpb/macos-arm64/deployment-target.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/deployment-target.txt',
                                                                          '850e86887e4cf6dd89bb3318cab398ab67d3f2e2adc70a4bd20de3ee7e3c8960'),
 'selected/provenance/canonical-mcpb/macos-arm64/libraries.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/libraries.txt',
                                                                  '4214619b869655132ee3ae1fd4c436a9cf602ba89ed6fc85f91debf28afd0cd6'),
 'selected/provenance/canonical-mcpb/macos-arm64/rustc.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/rustc.txt',
                                                              'e0a03bccf22d7879cab5f3dffa6589dd425c6085ccb22507e3d2e2e8c60f9148'),
 'selected/provenance/canonical-mcpb/macos-arm64/toolkit-package-inspect-build.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/macos-arm64/toolkit-package-inspect-build.json',
                                                                                       'b19fa8d73dc63dac6ed7629987865589d2ea0bb9a86b31f51f2a43f02fd64463'),
 'selected/provenance/canonical-mcpb/source-preparation.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/source-preparation.json',
                                                                '3399419b90b24e352733b746b30c4be7194a578393e3a35047c76cc1974c17f8'),
 'selected/provenance/canonical-mcpb/source.patch': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/source.patch',
                                                     '8b313d93fdb40f0b5841caccce0f4c5ad96d7d3e052430edc1b67fb3d63569c3'),
 'selected/provenance/canonical-mcpb/source.tar.gz': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/source.tar.gz',
                                                      'f2655dd628014ac85e2a9257e6a7d8ac8b17c7f038b7b806e8bbbddb5ddccb66')})
TARGETS['linux-x86_64'].update({'edition': '20261009-library-support-selection',
 'guide': 'docs/verification/20261009-library-support-selection/linux-x86_64-release.md',
 'guide_sha256': '7a7ac3e09c4fea5ff2192feed823b03042f9f2bfd538445a49062f336d393631'})
PYTHON_KITS['linux-x86_64']["tool_updates"]["bin/toolkit-package-inspect"] = {'record': 'docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/toolkit-package-inspect-build.json',
 'sha256': '73941074e896a6a7aaead1959048dcf140ab9cbbebb44c7ebd1648af1c5deb10',
 'source': 'docs/verification/20261008-mcpb-canonical-delivery/linux/build-delivery/toolkit-package-inspect'}
PYTHON_KITS['linux-x86_64']["sources"].update({'selected/provenance/canonical-mcpb/Cargo.lock': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/Cargo.lock',
                                                   'f911a612fd54bd1634e9a1d36a955198723779c2fcd43efded9acb82885f357a'),
 'selected/provenance/canonical-mcpb/crates/adler2-2.0.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/adler2-2.0.1.crate',
                                                                  '320119579fcad9c21884f5c4861d16174d0e06250625266f50fe6898340abefa'),
 'selected/provenance/canonical-mcpb/crates/ambient-authority-0.0.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/ambient-authority-0.0.2.crate',
                                                                             'e9d4ee0d472d1cd2e28c97dfa124b3d8d992e10eb0a035f33f5d12e3a177ba3b'),
 'selected/provenance/canonical-mcpb/crates/bitflags-2.13.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/bitflags-2.13.2.crate',
                                                                     '3ded4057c258ba199e2d26386d3af3780957ecaee6c4ef4041c6b4b8b97c0b06'),
 'selected/provenance/canonical-mcpb/crates/block-buffer-0.10.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/block-buffer-0.10.4.crate',
                                                                         '3078c7629b62d3f0439517fa394996acacc5cbc91c5a20d8c658e77abd503a71'),
 'selected/provenance/canonical-mcpb/crates/cap-primitives-4.0.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cap-primitives-4.0.3.crate',
                                                                          '8b5f74729fd2f44701d1a8eb47e906cdb3ccd9ec0f02baad85a744b791940b18'),
 'selected/provenance/canonical-mcpb/crates/cap-std-4.0.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cap-std-4.0.3.crate',
                                                                   'c1ec78e242cfa2cfe276807ac2ecc00315a6c97786977414bcd1c3963b6c91b8'),
 'selected/provenance/canonical-mcpb/crates/cfg-if-1.0.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cfg-if-1.0.5.crate',
                                                                  '4e7648175b45a9a48536d676f68d918270699102aa8dab5496df06904c914600'),
 'selected/provenance/canonical-mcpb/crates/cpufeatures-0.2.17.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/cpufeatures-0.2.17.crate',
                                                                        '59ed5838eebb26a2bb2e58f6d5b5316989ae9d08bab10e0e6d103e656d1b0280'),
 'selected/provenance/canonical-mcpb/crates/crc32fast-1.5.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/crc32fast-1.5.2.crate',
                                                                     '01a7799fd6b852db0e61728dde9a204c423b44d689dbd432522543614b490e78'),
 'selected/provenance/canonical-mcpb/crates/crypto-common-0.1.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/crypto-common-0.1.7.crate',
                                                                         '78c8292055d1c1df0cce5d180393dc8cce0abec0a7102adb6c7b1eef6016d60a'),
 'selected/provenance/canonical-mcpb/crates/digest-0.10.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/digest-0.10.7.crate',
                                                                   '9ed9a281f7bc9b7576e61468ba615a66a5c8cfdff42420a70aa82701a3b1e292'),
 'selected/provenance/canonical-mcpb/crates/displaydoc-0.2.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/displaydoc-0.2.7.crate',
                                                                      'c6232dd377dcc64799954cbd3a9bb882e9cdc1308ccd87b1c098f1fb2eaf82a8'),
 'selected/provenance/canonical-mcpb/crates/equivalent-1.0.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/equivalent-1.0.2.crate',
                                                                      '877a4ace8713b0bcf2a4e7eec82529c029f1d0619886d18145fea96c3ffe5c0f'),
 'selected/provenance/canonical-mcpb/crates/errno-0.3.14.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/errno-0.3.14.crate',
                                                                  '39cab71617ae0d63f51a36d69f866391735b51691dbda63cf6f96d042b63efeb'),
 'selected/provenance/canonical-mcpb/crates/flate2-1.1.10.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/flate2-1.1.10.crate',
                                                                   '6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb'),
 'selected/provenance/canonical-mcpb/crates/form_urlencoded-1.2.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/form_urlencoded-1.2.2.crate',
                                                                           'cb4cb245038516f5f85277875cdaa4f7d2c9a0fa0468de06ed190163b1581fcf'),
 'selected/provenance/canonical-mcpb/crates/fs-set-times-0.20.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/fs-set-times-0.20.3.crate',
                                                                         '94e7099f6313ecacbe1256e8ff9d617b75d1bcb16a6fddef94866d225a01a14a'),
 'selected/provenance/canonical-mcpb/crates/generic-array-0.14.7.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/generic-array-0.14.7.crate',
                                                                          '85649ca51fd72272d7821adaf274ad91c288277713d9c18820d8499a7ff69e9a'),
 'selected/provenance/canonical-mcpb/crates/hashbrown-0.17.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/hashbrown-0.17.1.crate',
                                                                      'ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a'),
 'selected/provenance/canonical-mcpb/crates/hex-0.4.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/hex-0.4.3.crate',
                                                               '7f24254aa9a54b5c858eaee2f5bccdb46aaf0e486a595ed5fd8f86ba55232a70'),
 'selected/provenance/canonical-mcpb/crates/icu_collections-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_collections-2.3.0.crate',
                                                                           'fa68d21081c4a05d5a901a1c62add574c77048b6a1c67be3b50ce0b60d4ca513'),
 'selected/provenance/canonical-mcpb/crates/icu_locale_core-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_locale_core-2.3.0.crate',
                                                                           'd56e28588da92eee5c3201a6eff33fabdd49b62269c8938d4ff050ce4d900deb'),
 'selected/provenance/canonical-mcpb/crates/icu_normalizer-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_normalizer-2.3.0.crate',
                                                                          '12f9cf5f235641ed274641dd81c3f28d870e276763d0797aeeab72317b1c646f'),
 'selected/provenance/canonical-mcpb/crates/icu_normalizer_data-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_normalizer_data-2.3.0.crate',
                                                                               '1563da1ed3e0b3bf3d74c9b85917ac9c56464d2f57242270c09c9e752f8021a0'),
 'selected/provenance/canonical-mcpb/crates/icu_properties-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_properties-2.3.0.crate',
                                                                          '7e7ca276ad3145661a65914e6daf131ca5120cd3dcee8f8f3214b8875184a148'),
 'selected/provenance/canonical-mcpb/crates/icu_properties_data-2.3.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_properties_data-2.3.0.crate',
                                                                               'e590f038c1464a96894fd6d10127e90a8be4509f56ff7ecef851b15cee0b7caa'),
 'selected/provenance/canonical-mcpb/crates/icu_provider-2.3.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/icu_provider-2.3.1.crate',
                                                                        'd27bbb9d3abbefac45d55f647c9de1d44aafcd1186eb91879afef17c396c3e73'),
 'selected/provenance/canonical-mcpb/crates/idna-1.1.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/idna-1.1.0.crate',
                                                                '3b0875f23caa03898994f6ddc501886a45c7d3d62d04d2d90788d47be1b1e4de'),
 'selected/provenance/canonical-mcpb/crates/idna_adapter-1.2.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/idna_adapter-1.2.2.crate',
                                                                        'cb68373c0d6620ef8105e855e7745e18b0d00d3bdb07fb532e434244cdb9a714'),
 'selected/provenance/canonical-mcpb/crates/indexmap-2.14.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/indexmap-2.14.2.crate',
                                                                     'cc4e190f5d26ca7051642629da2c52fc03bde85a03197c99408dcd291734c855'),
 'selected/provenance/canonical-mcpb/crates/io-extras-0.19.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/io-extras-0.19.0.crate',
                                                                      '20fd6de4ccfcc187e38bc21cfa543cb5a302cb86a8b114eb7f0bf0dc9f8ac00f'),
 'selected/provenance/canonical-mcpb/crates/io-lifetimes-2.0.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/io-lifetimes-2.0.4.crate',
                                                                        '06432fb54d3be7964ecd3649233cddf80db2832f47fec34c01f65b3d9d774983'),
 'selected/provenance/canonical-mcpb/crates/io-lifetimes-3.0.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/io-lifetimes-3.0.1.crate',
                                                                        '2f0fb0570afe1fed943c5c3d4102d5358592d8625fda6a0007fdbe65a92fba96'),
 'selected/provenance/canonical-mcpb/crates/ipnet-2.12.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/ipnet-2.12.2.crate',
                                                                  '791930b43c0d5973160d90a8f3894509f2b273430f5c5c73b668636d0287c5c0'),
 'selected/provenance/canonical-mcpb/crates/itoa-1.0.18.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/itoa-1.0.18.crate',
                                                                 '8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682'),
 'selected/provenance/canonical-mcpb/crates/libc-0.2.189.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/libc-0.2.189.crate',
                                                                  '3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2'),
 'selected/provenance/canonical-mcpb/crates/linux-raw-sys-0.12.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/linux-raw-sys-0.12.1.crate',
                                                                          '32a66949e030da00e8c7d4434b251670a91556f4144941d37452769c25d58a53'),
 'selected/provenance/canonical-mcpb/crates/litemap-0.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/litemap-0.8.3.crate',
                                                                   '47d9d19d1d6efa0109d2f65ff4c85cddd50bd572e5a00127ab10987290bcefae'),
 'selected/provenance/canonical-mcpb/crates/maybe-owned-0.3.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/maybe-owned-0.3.4.crate',
                                                                       '4facc753ae494aeb6e3c22f839b158aebd4f9270f55cd3c79906c45476c47ab4'),
 'selected/provenance/canonical-mcpb/crates/memchr-2.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/memchr-2.8.3.crate',
                                                                  'cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98'),
 'selected/provenance/canonical-mcpb/crates/miniz_oxide-0.9.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/miniz_oxide-0.9.1.crate',
                                                                       'b63fbc4a50860e98e7b2aa7804ded1db5cbc3aff9193adaff57a6931bf7c4b4c'),
 'selected/provenance/canonical-mcpb/crates/once_cell-1.21.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/once_cell-1.21.4.crate',
                                                                      '9f7c3e4beb33f85d45ae3e3a1792185706c8e16d043238c593331cc7cd313b50'),
 'selected/provenance/canonical-mcpb/crates/percent-encoding-2.3.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/percent-encoding-2.3.2.crate',
                                                                            '9b4f627cb1b25917193a259e49bdad08f671f8d9708acfd5fe0a8c1455d87220'),
 'selected/provenance/canonical-mcpb/crates/potential_utf-0.1.6.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/potential_utf-0.1.6.crate',
                                                                         'd83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661'),
 'selected/provenance/canonical-mcpb/crates/proc-macro2-1.0.107.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/proc-macro2-1.0.107.crate',
                                                                         '985e7ec9bb745e6ce6535b544d84d6cd6f7ad8bd711c398938ae983b91a766d9'),
 'selected/provenance/canonical-mcpb/crates/quote-1.0.47.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/quote-1.0.47.crate',
                                                                  '1fbf4db142a473a8d80c26bbf18454ed458bf8d26c8219c331daecfdbd079001'),
 'selected/provenance/canonical-mcpb/crates/rustix-1.1.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/rustix-1.1.5.crate',
                                                                  '891efababe418670775f199f0d233d84843c227a0949a883ce15b37c78d6629d'),
 'selected/provenance/canonical-mcpb/crates/rustix-linux-procfs-0.1.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/rustix-linux-procfs-0.1.1.crate',
                                                                               '2fc84bf7e9aa16c4f2c758f27412dc9841341e16aa682d9c7ac308fe3ee12056'),
 'selected/provenance/canonical-mcpb/crates/serde-1.0.229.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde-1.0.229.crate',
                                                                   '4148590afebada386688f18773da617792bf2ef03ffc1e4cbd2b1d45b023e0ba'),
 'selected/provenance/canonical-mcpb/crates/serde_core-1.0.229.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_core-1.0.229.crate',
                                                                        '67dca2c9c51e58a4791a4b1ed58308b39c64224d349a935ab5039aa360942a48'),
 'selected/provenance/canonical-mcpb/crates/serde_derive-1.0.229.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_derive-1.0.229.crate',
                                                                          'e7a5d71263a5a7d47b41f6b3f06ba276f10cc18b0931f1799f710578e2309348'),
 'selected/provenance/canonical-mcpb/crates/serde_json-1.0.151.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_json-1.0.151.crate',
                                                                        'c841b55ecdae098c80dcae9cf767f6f8a0c2cdb3416bbef72181df4d0fe73f14'),
 'selected/provenance/canonical-mcpb/crates/serde_path_to_error-0.1.20.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/serde_path_to_error-0.1.20.crate',
                                                                                '10a9ff822e371bb5403e391ecd83e182e0e77ba7f6fe0160b795797109d1b457'),
 'selected/provenance/canonical-mcpb/crates/sha2-0.10.9.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/sha2-0.10.9.crate',
                                                                 'a7507d819769d01a365ab707794a4084392c824f54a7a6a7862f8c3d0892b283'),
 'selected/provenance/canonical-mcpb/crates/simd-adler32-0.3.10.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/simd-adler32-0.3.10.crate',
                                                                         '3a219298ac11a56ea9a6d2120044824d6f01aeb034955e7af7bc16858527deea'),
 'selected/provenance/canonical-mcpb/crates/smallvec-1.16.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/smallvec-1.16.2.crate',
                                                                     'f9395f0f0eee849a9b707b2f06bb92a6a422090e2123bb2ef8e87a0e61892a8e'),
 'selected/provenance/canonical-mcpb/crates/stable_deref_trait-1.2.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/stable_deref_trait-1.2.1.crate',
                                                                              '6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596'),
 'selected/provenance/canonical-mcpb/crates/syn-3.0.6.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/syn-3.0.6.crate',
                                                               '8593e8e72159ed2257d083c7a454a85cbf854f37a0966d8d483aff8c8a3ebcee'),
 'selected/provenance/canonical-mcpb/crates/synstructure-0.14.0.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/synstructure-0.14.0.crate',
                                                                         '901704edd0dfe137f1987838ee4f259e4e063c31371bdb423f7ae38ec6f77f02'),
 'selected/provenance/canonical-mcpb/crates/thiserror-2.0.21.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/thiserror-2.0.21.crate',
                                                                      '09e52cb86a36cede5cb101bf8908837b3e4c6e5e59fe7fd85c23fb56200d189e'),
 'selected/provenance/canonical-mcpb/crates/thiserror-impl-2.0.21.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/thiserror-impl-2.0.21.crate',
                                                                           'fe5197923287db20a58125f0bc85c062f7f2c892de97b18c356f9efb14b28524'),
 'selected/provenance/canonical-mcpb/crates/tinystr-0.8.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/tinystr-0.8.4.crate',
                                                                   'b1e27c91459209c2986af3dcf603a5a74a4368754ce37414f59acc971167f643'),
 'selected/provenance/canonical-mcpb/crates/typenum-1.20.1.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/typenum-1.20.1.crate',
                                                                    'b6f5e870be6c3b371b77fe0ee0bafb859fa4964b4404c27de1d380043c4dda20'),
 'selected/provenance/canonical-mcpb/crates/unicode-ident-1.0.26.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/unicode-ident-1.0.26.crate',
                                                                          'd245f478577f809a851594d02313b640fb437e0bb33866753cff937863096954'),
 'selected/provenance/canonical-mcpb/crates/url-2.5.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/url-2.5.8.crate',
                                                               'ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed'),
 'selected/provenance/canonical-mcpb/crates/utf8_iter-1.0.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/utf8_iter-1.0.4.crate',
                                                                     'b6c140620e7ffbb22c2dee59cafe6084a59b5ffc27a8859a5f0d494b5d52b6be'),
 'selected/provenance/canonical-mcpb/crates/version_check-0.9.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/version_check-0.9.5.crate',
                                                                         '0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a'),
 'selected/provenance/canonical-mcpb/crates/writeable-0.6.4.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/writeable-0.6.4.crate',
                                                                     '3ad82d2a33cdc9674dc7465672f271e096168fcdbe0f799d9e6db8c5892679dc'),
 'selected/provenance/canonical-mcpb/crates/yoke-0.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/yoke-0.8.3.crate',
                                                                '709fe23a0424b6a435d82152b1bd3fdfb0833487d5fa90d05d42762a9891fef5'),
 'selected/provenance/canonical-mcpb/crates/yoke-derive-0.8.3.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/yoke-derive-0.8.3.crate',
                                                                       '33811428bee40dbceb6d545e95754741d17a6aef9a4849f0fd62e2ba4f412a78'),
 'selected/provenance/canonical-mcpb/crates/zerofrom-0.1.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerofrom-0.1.8.crate',
                                                                    '0ec05a11813ea801ff6d75110ad09cd0824ddba17dfe17128ea0d5f68e6c5272'),
 'selected/provenance/canonical-mcpb/crates/zerofrom-derive-0.1.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerofrom-derive-0.1.8.crate',
                                                                           'f75b4683f6c7f45248d4d64056a24298c6281e0993356d7d1b4a1a962ef10d4a'),
 'selected/provenance/canonical-mcpb/crates/zerotrie-0.2.5.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerotrie-0.2.5.crate',
                                                                    '4ea269c3bd32f0a32c321907a2ae912ba6f4649bb0fc764a15627e99a7095a3f'),
 'selected/provenance/canonical-mcpb/crates/zerovec-0.11.8.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerovec-0.11.8.crate',
                                                                    'bb0464e17806c1d976d5cba29399c7f08e516e279e2ba493f63123b5fca67dd8'),
 'selected/provenance/canonical-mcpb/crates/zerovec-derive-0.11.6.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zerovec-derive-0.11.6.crate',
                                                                           '34df6fc39dbd26ddc9c10e6a2984476e13acce22e64e4487636ef494369225da'),
 'selected/provenance/canonical-mcpb/crates/zip-2.4.2.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zip-2.4.2.crate',
                                                               'fabe6324e908f85a1c52063ce7aa26b68dcb7eb6dbc83a2d148403c9bc3eba50'),
 'selected/provenance/canonical-mcpb/crates/zmij-1.0.23.crate': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/crates/zmij-1.0.23.crate',
                                                                 '29666d0abbfad1e3dc4dcf6144730dd3a3ab225bbbdac83319345b1b44ccfc1b'),
 'selected/provenance/canonical-mcpb/licenses/adler2-2.0.1/LICENSE-0BSD': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/adler2-2.0.1/LICENSE-0BSD',
                                                                           '861399f8c21c042b110517e76dc6b63a2b334276c8cf17412fc3c8908ca8dc17'),
 'selected/provenance/canonical-mcpb/licenses/adler2-2.0.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/adler2-2.0.1/LICENSE-APACHE',
                                                                             '8ada45cd9f843acf64e4722ae262c622a2b3b3007c7310ef36ac1061a30f6adb'),
 'selected/provenance/canonical-mcpb/licenses/adler2-2.0.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/adler2-2.0.1/LICENSE-MIT',
                                                                          '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/COPYRIGHT',
                                                                                   'd00d6d48df2db6f91e4ca9c656e5e30782d5becf52bb3e5bfd0aff467bd37781'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/LICENSE-APACHE',
                                                                                        'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/ambient-authority-0.0.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ambient-authority-0.0.2/LICENSE-MIT',
                                                                                     '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/bitflags-2.13.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/bitflags-2.13.2/LICENSE-APACHE',
                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/bitflags-2.13.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/bitflags-2.13.2/LICENSE-MIT',
                                                                             '6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb'),
 'selected/provenance/canonical-mcpb/licenses/block-buffer-0.10.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/block-buffer-0.10.4/LICENSE-APACHE',
                                                                                    'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/block-buffer-0.10.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/block-buffer-0.10.4/LICENSE-MIT',
                                                                                 'd5c22aa3118d240e877ad41c5d9fa232f9c77d757d4aac0c2f943afc0a95e0ef'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/COPYRIGHT',
                                                                                'a8799c2a07dabcb8d52481f885c5ab7000acb8a4d3cd9503fbcb97c84d350e65'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/LICENSE-APACHE',
                                                                                     'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                             '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/cap-primitives-4.0.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-primitives-4.0.3/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/COPYRIGHT',
                                                                         '6b1691fdee3b03c3946696a7be125fcf50eff310b9486fac850c1b8aea91478b'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/LICENSE-APACHE',
                                                                              'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                      '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/cap-std-4.0.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cap-std-4.0.3/LICENSE-MIT',
                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/cfg-if-1.0.5/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cfg-if-1.0.5/LICENSE-APACHE',
                                                                             'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/cfg-if-1.0.5/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cfg-if-1.0.5/LICENSE-MIT',
                                                                          '378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397'),
 'selected/provenance/canonical-mcpb/licenses/cpufeatures-0.2.17/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cpufeatures-0.2.17/LICENSE-APACHE',
                                                                                   'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/cpufeatures-0.2.17/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/cpufeatures-0.2.17/LICENSE-MIT',
                                                                                'ae9baa7beea910273c2f384c2a6b721fb7bd02bda3436074a1072e4ee689f985'),
 'selected/provenance/canonical-mcpb/licenses/crc32fast-1.5.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crc32fast-1.5.2/LICENSE-APACHE',
                                                                                'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
 'selected/provenance/canonical-mcpb/licenses/crc32fast-1.5.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crc32fast-1.5.2/LICENSE-MIT',
                                                                             '61d383b05b87d78f94d2937e2580cce47226d17823c0430fbcad09596537efcf'),
 'selected/provenance/canonical-mcpb/licenses/crypto-common-0.1.7/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crypto-common-0.1.7/LICENSE-APACHE',
                                                                                    'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/crypto-common-0.1.7/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/crypto-common-0.1.7/LICENSE-MIT',
                                                                                 '3521672491a3479422d5fe1aca6645dd2984090f85da6e5205abfb18fb7a6897'),
 'selected/provenance/canonical-mcpb/licenses/digest-0.10.7/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/digest-0.10.7/LICENSE-APACHE',
                                                                              'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/digest-0.10.7/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/digest-0.10.7/LICENSE-MIT',
                                                                           '9e0dfd2dd4173a530e238cb6adb37aa78c34c6bc7444e0e10c1ab5d8881f63ba'),
 'selected/provenance/canonical-mcpb/licenses/displaydoc-0.2.7/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/displaydoc-0.2.7/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/displaydoc-0.2.7/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/displaydoc-0.2.7/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/equivalent-1.0.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/equivalent-1.0.2/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/equivalent-1.0.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/equivalent-1.0.2/LICENSE-MIT',
                                                                              '7365cc8878a1d7ce155a58c4ca09c3d7a6be413efa5334a80ea842912b669349'),
 'selected/provenance/canonical-mcpb/licenses/errno-0.3.14/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/errno-0.3.14/LICENSE-APACHE',
                                                                             'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/errno-0.3.14/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/errno-0.3.14/LICENSE-MIT',
                                                                          '8764a597675778ddfd4e25f81b08a05dbcf089ac05662df7613fe67f150e3aa2'),
 'selected/provenance/canonical-mcpb/licenses/flate2-1.1.10/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/flate2-1.1.10/LICENSE-APACHE',
                                                                              'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/flate2-1.1.10/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/flate2-1.1.10/LICENSE-MIT',
                                                                           '025436edff4cfcdde17a5811fdea78892d8482efd1abdec5a17872d07a4f2112'),
 'selected/provenance/canonical-mcpb/licenses/form_urlencoded-1.2.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/form_urlencoded-1.2.2/LICENSE-APACHE',
                                                                                      'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/form_urlencoded-1.2.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/form_urlencoded-1.2.2/LICENSE-MIT',
                                                                                   '20c7855c364d57ea4c97889a5e8d98470a9952dade37bd9248b9a54431670e5e'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/COPYRIGHT',
                                                                               'e196ffeb5ab101e9ec8bbe395c9f367c5380ee2d28c47e5c03371523798a9af7'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/LICENSE-APACHE',
                                                                                    'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                            '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/fs-set-times-0.20.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/fs-set-times-0.20.3/LICENSE-MIT',
                                                                                 '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/generic-array-0.14.7/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/generic-array-0.14.7/LICENSE',
                                                                              'c09aae9d3c77b531f56351a9947bc7446511d6b025b3255312d3e3442a9a7583'),
 'selected/provenance/canonical-mcpb/licenses/hashbrown-0.17.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hashbrown-0.17.1/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/hashbrown-0.17.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hashbrown-0.17.1/LICENSE-MIT',
                                                                              'ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2'),
 'selected/provenance/canonical-mcpb/licenses/hex-0.4.3/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hex-0.4.3/LICENSE-APACHE',
                                                                          'c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08'),
 'selected/provenance/canonical-mcpb/licenses/hex-0.4.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/hex-0.4.3/LICENSE-MIT',
                                                                       'f7bdb3426d045cd50efd4953026e3eb5a83d0199f458a075602611b9344da5b9'),
 'selected/provenance/canonical-mcpb/licenses/icu_collections-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_collections-2.3.0/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_locale_core-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_locale_core-2.3.0/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_normalizer-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_normalizer-2.3.0/LICENSE',
                                                                              'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_normalizer_data-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_normalizer_data-2.3.0/LICENSE',
                                                                                   'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_properties-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_properties-2.3.0/LICENSE',
                                                                              'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_properties_data-2.3.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_properties_data-2.3.0/LICENSE',
                                                                                   'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/icu_provider-2.3.1/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/icu_provider-2.3.1/LICENSE',
                                                                            'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/idna-1.1.0/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna-1.1.0/LICENSE-APACHE',
                                                                           'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/idna-1.1.0/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna-1.1.0/LICENSE-MIT',
                                                                        'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
 'selected/provenance/canonical-mcpb/licenses/idna_adapter-1.2.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna_adapter-1.2.2/LICENSE-APACHE',
                                                                                   'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/idna_adapter-1.2.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/idna_adapter-1.2.2/LICENSE-MIT',
                                                                                '8b43ce8accd61e9d370b5ca9e9c4f953279b5c239926c62315b40e24df51b726'),
 'selected/provenance/canonical-mcpb/licenses/indexmap-2.14.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/indexmap-2.14.2/LICENSE-APACHE',
                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/indexmap-2.14.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/indexmap-2.14.2/LICENSE-MIT',
                                                                             'ecc269ef87fd38a1d98e30bfac9ba964a9dbd9315c3770fed98d4d7cb5882055'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/COPYRIGHT',
                                                                            'f0c1d22de8bd3b776a40f2691f08b84e91c7bafc78b72a51150e231002b88db3'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                         '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/io-extras-0.19.0/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-extras-0.19.0/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/COPYRIGHT',
                                                                              '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/LICENSE-APACHE',
                                                                                   'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                           '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-2.0.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-2.0.4/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/COPYRIGHT',
                                                                              '495c30b45120f8af07cfa26eb9cb1ebfe8324560dca2b3b1e76cad1b9b6b489a'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/LICENSE-APACHE',
                                                                                   'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                           '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/io-lifetimes-3.0.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/io-lifetimes-3.0.1/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/ipnet-2.12.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ipnet-2.12.2/LICENSE-APACHE',
                                                                             '87d9feb9238c6bd8e0024fc4733b06cff036f89f36d93b7df1c8a0549bbb7a5b'),
 'selected/provenance/canonical-mcpb/licenses/ipnet-2.12.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/ipnet-2.12.2/LICENSE-MIT',
                                                                          '47dc9ff29128ddfb4d6a0435383c9f89120bc374dbcc1dd00b933a0b28aa7865'),
 'selected/provenance/canonical-mcpb/licenses/itoa-1.0.18/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/itoa-1.0.18/LICENSE-APACHE',
                                                                            '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/itoa-1.0.18/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/itoa-1.0.18/LICENSE-MIT',
                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/libc-0.2.189/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/libc-0.2.189/LICENSE-APACHE',
                                                                             '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/libc-0.2.189/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/libc-0.2.189/LICENSE-MIT',
                                                                          '123a331b5dbf04c30097fa43b8f858bc85df671fe776de498d01f3d6b7c1f69e'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/COPYRIGHT',
                                                                                '3290ae0fbc9ddb77d2239121d710f0bb9d31b3b4744e6d97fe01e652b4c1870b'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-APACHE',
                                                                                     'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                             '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/linux-raw-sys-0.12.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/linux-raw-sys-0.12.1/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/litemap-0.8.3/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/litemap-0.8.3/LICENSE',
                                                                       'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/maybe-owned-0.3.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/maybe-owned-0.3.4/LICENSE-APACHE',
                                                                                  'aecc100d0547aa3bcbe04bcf5cb87c7db0be11004602adfa5056edf8bb847b35'),
 'selected/provenance/canonical-mcpb/licenses/maybe-owned-0.3.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/maybe-owned-0.3.4/LICENSE-MIT',
                                                                               'ef2b3bbd7b718a7881253226313f103e89b379e1279681a3c1ebcb65a27d209d'),
 'selected/provenance/canonical-mcpb/licenses/memchr-2.8.3/COPYING': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/memchr-2.8.3/COPYING',
                                                                      '01c266bced4a434da0051174d6bee16a4c82cf634e2679b6155d40d75012390f'),
 'selected/provenance/canonical-mcpb/licenses/memchr-2.8.3/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/memchr-2.8.3/LICENSE-MIT',
                                                                          '0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE',
                                                                           '4108245a1f2df9d4e94df8abed5b4ba0759bb2f9b40a6b939f1be141077ae50b'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE-APACHE.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE-APACHE.md',
                                                                                     '0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE-MIT.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE-MIT.md',
                                                                                  '799e9ca9d179295ef372f25d3769cdda7d25bb2668add6a6a1e22d1e4c678b8d'),
 'selected/provenance/canonical-mcpb/licenses/miniz_oxide-0.9.1/LICENSE-ZLIB.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/miniz_oxide-0.9.1/LICENSE-ZLIB.md',
                                                                                   '0a54e647fe54104658b5e563c04c6f9edf251710e47bce692e0bd990a4ddaa39'),
 'selected/provenance/canonical-mcpb/licenses/once_cell-1.21.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/once_cell-1.21.4/LICENSE-APACHE',
                                                                                 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/once_cell-1.21.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/once_cell-1.21.4/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/percent-encoding-2.3.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/percent-encoding-2.3.2/LICENSE-APACHE',
                                                                                       'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/percent-encoding-2.3.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/percent-encoding-2.3.2/LICENSE-MIT',
                                                                                    'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
 'selected/provenance/canonical-mcpb/licenses/potential_utf-0.1.6/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/potential_utf-0.1.6/LICENSE',
                                                                             'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/proc-macro2-1.0.107/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/proc-macro2-1.0.107/LICENSE-APACHE',
                                                                                    '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/proc-macro2-1.0.107/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/proc-macro2-1.0.107/LICENSE-MIT',
                                                                                 '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/quote-1.0.47/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/quote-1.0.47/LICENSE-APACHE',
                                                                             '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/quote-1.0.47/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/quote-1.0.47/LICENSE-MIT',
                                                                          '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/COPYRIGHT',
                                                                        '377c2e7c53250cc5905c0b0532d35973392af16ffb9596a41d99d202cf3617c9'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/LICENSE-APACHE',
                                                                             'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                     '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/rustix-1.1.5/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-1.1.5/LICENSE-MIT',
                                                                          '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/COPYRIGHT',
                                                                                     'd415a86ccfd794129666247cbbadebee7a6d0ac3ff701c76c07b0f9424f0307d'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-APACHE',
                                                                                          'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-Apache-2.0_WITH_LLVM-exception',
                                                                                                                  '268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5'),
 'selected/provenance/canonical-mcpb/licenses/rustix-linux-procfs-0.1.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/rustix-linux-procfs-0.1.1/LICENSE-MIT',
                                                                                       '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde-1.0.229/LICENSE-APACHE',
                                                                              '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde-1.0.229/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde-1.0.229/LICENSE-MIT',
                                                                           '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_core-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_core-1.0.229/LICENSE-APACHE',
                                                                                   '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_core-1.0.229/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_core-1.0.229/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_derive-1.0.229/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_derive-1.0.229/LICENSE-APACHE',
                                                                                     '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_derive-1.0.229/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_derive-1.0.229/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_json-1.0.151/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_json-1.0.151/LICENSE-APACHE',
                                                                                   '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_json-1.0.151/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_json-1.0.151/LICENSE-MIT',
                                                                                '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/serde_path_to_error-0.1.20/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_path_to_error-0.1.20/LICENSE-APACHE',
                                                                                           '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/serde_path_to_error-0.1.20/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/serde_path_to_error-0.1.20/LICENSE-MIT',
                                                                                        '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/sha2-0.10.9/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/sha2-0.10.9/LICENSE-APACHE',
                                                                            'a9040321c3712d8fd0b09cf52b17445de04a23a10165049ae187cd39e5c86be5'),
 'selected/provenance/canonical-mcpb/licenses/sha2-0.10.9/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/sha2-0.10.9/LICENSE-MIT',
                                                                         'b4eb00df6e2a4d22518fcaa6a2b4646f249b3a3c9814509b22bd2091f1392ff1'),
 'selected/provenance/canonical-mcpb/licenses/simd-adler32-0.3.10/LICENSE.md': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/simd-adler32-0.3.10/LICENSE.md',
                                                                                '42a35170233e83e18856792e748de4c1ce4a63b2afce9a370c89ef3fe23f9f2d'),
 'selected/provenance/canonical-mcpb/licenses/smallvec-1.16.2/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/smallvec-1.16.2/LICENSE-APACHE',
                                                                                'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/smallvec-1.16.2/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/smallvec-1.16.2/LICENSE-MIT',
                                                                             '0b28172679e0009b655da42797c03fd163a3379d5cfa67ba1f1655e974a2a1a9'),
 'selected/provenance/canonical-mcpb/licenses/stable_deref_trait-1.2.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/stable_deref_trait-1.2.1/LICENSE-APACHE',
                                                                                         'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/stable_deref_trait-1.2.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/stable_deref_trait-1.2.1/LICENSE-MIT',
                                                                                      '5e05b024f653a5ce199e77cbbbd42fb5553562ec714b819421ed0c3e552a75d7'),
 'selected/provenance/canonical-mcpb/licenses/syn-3.0.6/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/syn-3.0.6/LICENSE-APACHE',
                                                                          '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/syn-3.0.6/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/syn-3.0.6/LICENSE-MIT',
                                                                       '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/synstructure-0.14.0/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/synstructure-0.14.0/LICENSE',
                                                                             '219920e865eee70b7dcfc948a86b099e7f4fe2de01bcca2ca9a20c0a033f2b59'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-2.0.21/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-2.0.21/LICENSE-APACHE',
                                                                                 '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-2.0.21/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-2.0.21/LICENSE-MIT',
                                                                              '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-impl-2.0.21/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-impl-2.0.21/LICENSE-APACHE',
                                                                                      '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/thiserror-impl-2.0.21/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/thiserror-impl-2.0.21/LICENSE-MIT',
                                                                                   '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/tinystr-0.8.4/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/tinystr-0.8.4/LICENSE',
                                                                       'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/typenum-1.20.1/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/typenum-1.20.1/LICENSE',
                                                                        'db11fec9946737df39ca3898d9cd8c10ec6f6c3a884a6802b0ad0b81b4e8f23a'),
 'selected/provenance/canonical-mcpb/licenses/typenum-1.20.1/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/typenum-1.20.1/LICENSE-APACHE',
                                                                               '516b24e051bf5630880ebbd55c40a25ce9552ebaf8970a53e8976eb70e522406'),
 'selected/provenance/canonical-mcpb/licenses/typenum-1.20.1/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/typenum-1.20.1/LICENSE-MIT',
                                                                            'a825bd853ab71619a4923d7b4311221427848070ff44d990da39b0b274c1683f'),
 'selected/provenance/canonical-mcpb/licenses/unicode-ident-1.0.26/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/unicode-ident-1.0.26/LICENSE-APACHE',
                                                                                     '62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a'),
 'selected/provenance/canonical-mcpb/licenses/unicode-ident-1.0.26/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/unicode-ident-1.0.26/LICENSE-MIT',
                                                                                  '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/licenses/unicode-ident-1.0.26/LICENSE-UNICODE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/unicode-ident-1.0.26/LICENSE-UNICODE',
                                                                                      'f7db81051789b729fea528a63ec4c938fdcb93d9d61d97dc8cc2e9df6d47f2a1'),
 'selected/provenance/canonical-mcpb/licenses/url-2.5.8/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/url-2.5.8/LICENSE-APACHE',
                                                                          'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/url-2.5.8/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/url-2.5.8/LICENSE-MIT',
                                                                       'b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5'),
 'selected/provenance/canonical-mcpb/licenses/utf8_iter-1.0.4/COPYRIGHT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/utf8_iter-1.0.4/COPYRIGHT',
                                                                           'c30152c94a6d75e021adbc52b3a52470366a46edb917e17deae3259251af244c'),
 'selected/provenance/canonical-mcpb/licenses/utf8_iter-1.0.4/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/utf8_iter-1.0.4/LICENSE-APACHE',
                                                                                'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'),
 'selected/provenance/canonical-mcpb/licenses/utf8_iter-1.0.4/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/utf8_iter-1.0.4/LICENSE-MIT',
                                                                             '3fa4ca83dcc9237839b1bdeb2e6d16bdfb5ec0c5ce42b24694d8bbf0dcbef72c'),
 'selected/provenance/canonical-mcpb/licenses/version_check-0.9.5/LICENSE-APACHE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/version_check-0.9.5/LICENSE-APACHE',
                                                                                    'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'),
 'selected/provenance/canonical-mcpb/licenses/version_check-0.9.5/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/version_check-0.9.5/LICENSE-MIT',
                                                                                 'b7e650f3fce5c53249d1cdc608b54df156a97edd636cf9d23498d0cfe7aec63e'),
 'selected/provenance/canonical-mcpb/licenses/writeable-0.6.4/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/writeable-0.6.4/LICENSE',
                                                                         'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/yoke-0.8.3/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/yoke-0.8.3/LICENSE',
                                                                    'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/yoke-derive-0.8.3/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/yoke-derive-0.8.3/LICENSE',
                                                                           'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerofrom-0.1.8/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerofrom-0.1.8/LICENSE',
                                                                        'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerofrom-derive-0.1.8/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerofrom-derive-0.1.8/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerotrie-0.2.5/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerotrie-0.2.5/LICENSE',
                                                                        'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerovec-0.11.8/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerovec-0.11.8/LICENSE',
                                                                        'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zerovec-derive-0.11.6/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zerovec-derive-0.11.6/LICENSE',
                                                                               'f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2'),
 'selected/provenance/canonical-mcpb/licenses/zip-2.4.2/LICENSE': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zip-2.4.2/LICENSE',
                                                                   '04dc52136d82400b11df5f474949863a70a5d05d2278dec45b99c479494450a6'),
 'selected/provenance/canonical-mcpb/licenses/zmij-1.0.23/LICENSE-MIT': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/licenses/zmij-1.0.23/LICENSE-MIT',
                                                                         '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'),
 'selected/provenance/canonical-mcpb/linux-x86_64/build-result.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/build-result.json',
                                                                       '5959b730e84e7e5c47e0b6b7b0d7283f73339e22c4731e26325f3d3938d0a0b1'),
 'selected/provenance/canonical-mcpb/linux-x86_64/build.stdout': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/build.stdout',
                                                                  'beb0b30fafb04a0b9d665008c192b2d27408f65e5a280013f8d6ed80185a39d8'),
 'selected/provenance/canonical-mcpb/linux-x86_64/cargo.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/cargo.txt',
                                                               '9046ebb981a850fd1c3cef9bbc73aa7cea82ec651754a30e4c38b842b7f91f7a'),
 'selected/provenance/canonical-mcpb/linux-x86_64/compiler-os.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/compiler-os.txt',
                                                                     '59a77b5f2666d9c85c489bd1911a6eebbd91ef22fe48b90a3b75f1b21f3844d4'),
 'selected/provenance/canonical-mcpb/linux-x86_64/dependencies.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/dependencies.json',
                                                                       'fd0614386a008dba83617ff1da730163e78ba879f27e34b6fb173681390a2792'),
 'selected/provenance/canonical-mcpb/linux-x86_64/inspector-elf-versions.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/inspector-elf-versions.txt',
                                                                                '6ef43b80796e607aee5a4ce5ace5086feac15dfa3e4d9a03874f75ca2fae7094'),
 'selected/provenance/canonical-mcpb/linux-x86_64/inspector-ldd.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/inspector-ldd.txt',
                                                                       '069beb8cbe6dd88671cb25729353b95315c8ea5867546cc8277b51b205e4b089'),
 'selected/provenance/canonical-mcpb/linux-x86_64/rustc.txt': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/rustc.txt',
                                                               'a90721a3b031e0e1c8bfc34ece79b6608d2ae53166cfb77dad281a0eb5fa6bc8'),
 'selected/provenance/canonical-mcpb/linux-x86_64/toolchain.sha256': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/toolchain.sha256',
                                                                      'a9a439382571385cc2ab8438d5b253b1be668a011b688e994960605c03784342'),
 'selected/provenance/canonical-mcpb/linux-x86_64/toolkit-package-inspect-build.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/linux-x86_64/toolkit-package-inspect-build.json',
                                                                                        'b19157b19e2d3cee757592bc015532eb0d352135efa9babf1e92f15596988c70'),
 'selected/provenance/canonical-mcpb/source-preparation.json': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/source-preparation.json',
                                                                '3399419b90b24e352733b746b30c4be7194a578393e3a35047c76cc1974c17f8'),
 'selected/provenance/canonical-mcpb/source.patch': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/source.patch',
                                                     '8b313d93fdb40f0b5841caccce0f4c5ad96d7d3e052430edc1b67fb3d63569c3'),
 'selected/provenance/canonical-mcpb/source.tar.gz': ('docs/verification/20261008-mcpb-canonical-delivery/provenance/source.tar.gz',
                                                      'f2655dd628014ac85e2a9257e6a7d8ac8b17c7f038b7b806e8bbbddb5ddccb66')})


class PackageError(Exception):
    """A fixed, value-free packaging failure."""


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def relative_path(value: str) -> str:
    if not isinstance(value, str) or not value or len(value) > 1024:
        raise PackageError("unsafe_input_path")
    parts = value.split("/")
    if any(part in {"", ".", ".."} or not SAFE_PART.fullmatch(part) for part in parts):
        raise PackageError("unsafe_input_path")
    return value


def unique_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise PackageError("duplicate_metadata_key")
        result[key] = value
    return result


class Inputs:
    """Bound raw file reads without following descendant symlinks."""

    def __init__(self, root: Path):
        self.root = root
        self.total = 0

    def directory(self, path: str = "") -> int:
        fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            for part in relative_path(path).split("/") if path else []:
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                os.close(fd)
                fd = child
            return fd
        except BaseException:
            os.close(fd)
            raise

    def read(self, path: str, *, executable: bool = False) -> bytes:
        path = relative_path(path)
        parent, _, name = path.rpartition("/")
        directory = self.directory(parent)
        try:
            fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        finally:
            os.close(directory)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            mode = stat.S_IMODE(info.st_mode)
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                raise PackageError("input_not_regular_unlinked_file")
            if mode & 0o7000 or (mode != 0o755 if executable else bool(mode & 0o111)):
                raise PackageError("unexpected_input_mode")
            if info.st_size > MAX_FILE_BYTES:
                raise PackageError("input_too_large")
            data = stream.read(MAX_FILE_BYTES + 1)
        self.total += len(data)
        if len(data) > MAX_FILE_BYTES or self.total > MAX_TOTAL_BYTES:
            raise PackageError("input_too_large")
        return data

    def file_set(self, path: str) -> tuple[set[str], set[str]]:
        files: set[str] = set()
        directories: set[str] = set()

        def visit(fd: int, prefix: str) -> None:
            with os.scandir(fd) as entries:
                for entry in entries:
                    name = relative_path(prefix + entry.name)
                    info = entry.stat(follow_symlinks=False)
                    if stat.S_ISDIR(info.st_mode):
                        if stat.S_IMODE(info.st_mode) & 0o7000:
                            raise PackageError("unexpected_input_mode")
                        directories.add(name)
                        child = os.open(entry.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                        try:
                            visit(child, name + "/")
                        finally:
                            os.close(child)
                    elif stat.S_ISREG(info.st_mode):
                        files.add(name)
                    else:
                        raise PackageError("input_not_regular_unlinked_file")
                    if len(files) + len(directories) > MAX_ENTRIES:
                        raise PackageError("too_many_inputs")

        fd = self.directory(path)
        try:
            visit(fd, "")
        finally:
            os.close(fd)
        return files, directories


def setup_sources(selection: dict) -> dict:
    sources = dict(SOURCES)
    sources.update(selection.get("setup_assets", {}))
    sources["setup/backend.py"] = selection["setup_backend"]
    if "setup_adapter" in selection:
        sources["setup/mcp_adapter.py"] = selection["setup_adapter"]
    if "setup_installer" in selection:
        sources["setup/install-library-consumer.py"] = selection["setup_installer"]
    sources["README.md"] = (selection["guide"], selection["guide_sha256"])
    return sources


def selected_inputs(root: Path, target: str, path: Path | None, expected: str | None) -> dict:
    """Apply explicit overrides only to this target's existing named inputs.

    Omitted names retain their frozen pins. This does not bless changed source,
    add tools, or turn a supplied build record into execution qualification.
    """
    selection = copy.deepcopy(TARGETS[target])
    donor = copy.deepcopy(PYTHON_KITS[target])
    sources = setup_sources(selection)
    python_sources = PYTHON_SOURCES | donor.get("sources", {})
    settings = {"selection": selection, "donor": donor, "sources": sources,
                "python_sources": python_sources}
    if path is None and expected is None:
        return settings
    if (path is None or expected is None or not path.is_absolute()
            or not isinstance(expected, str) or not HEX.fullmatch(expected)):
        raise PackageError("invalid_input_selection")
    path = path.parent.resolve(strict=True) / path.name
    raw = Inputs(path.parent).read(path.name)
    if len(raw) > 1024 * 1024 or digest(raw) != expected:
        raise PackageError("input_selection_digest_mismatch")
    declared = json.loads(raw, object_pairs_hook=unique_object)
    if (not isinstance(declared, dict) or set(declared) != {
            "schema_version", "target", "edition", "consumer", "sources", "tools"}
            or type(declared["schema_version"]) is not int or declared["schema_version"] != 1
            or declared["target"] != target or not isinstance(declared["edition"], str)
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}", declared["edition"])):
        raise PackageError("invalid_input_selection")
    consumer = declared["consumer"]
    if (not isinstance(consumer, dict) or set(consumer) != {
            "bundle", "contents", "sums", "dependency_closure", "dependency_closure_sha256"}
            or any(not isinstance(consumer[key], str) or not HEX.fullmatch(consumer[key])
                   for key in ("contents", "sums", "dependency_closure_sha256"))):
        raise PackageError("invalid_consumer_selection")
    relative_path(consumer["bundle"])
    relative_path(consumer["dependency_closure"])
    selection.update(consumer)
    selection["edition"] = declared["edition"]
    overrides = declared["sources"]
    # This already shipped Python program is source, not a compiled tool with
    # a Cargo build record. Its old donor pin remains the default.
    installer_source = "selected/bin/install-library-consumer.py"
    # These shipped contracts may follow explicitly selected package planners.
    # Omission keeps the historical donor contract and its provenance intact.
    package_contracts = {"selected/contracts/PYTHON-INSTALL.md", "selected/contracts/NODE-INSTALL.md"}
    known_sources = sources.keys() | python_sources.keys() | {installer_source} | package_contracts
    if (not isinstance(overrides, dict) or not set(overrides) <= known_sources):
        raise PackageError("unknown_selected_source")
    for name, row in overrides.items():
        if (not isinstance(row, dict) or set(row) != {"path", "sha256"}
                or not isinstance(row["sha256"], str) or not HEX.fullmatch(row["sha256"])):
            raise PackageError("invalid_selected_source")
        value = (relative_path(row["path"]), row["sha256"])
        if name in sources or name == installer_source:
            sources[name] = value
        if name in python_sources or name in package_contracts:
            python_sources[name] = value
    tools = declared["tools"]
    known = donor["tools"].keys() | donor.get("tool_additions", {}).keys()
    if not isinstance(tools, dict) or not set(tools) <= known:
        raise PackageError("unknown_selected_tool")
    if installer_source in overrides and installer_source.removeprefix("selected/") in tools:
        raise PackageError("duplicate_selected_installer_source")
    for name, row in tools.items():
        if (not isinstance(row, dict) or set(row) != {"source", "sha256", "record", "record_sha256"}
                or any(not isinstance(row[key], str) or not HEX.fullmatch(row[key])
                       for key in ("sha256", "record_sha256"))):
            raise PackageError("invalid_selected_tool")
        source, record = relative_path(row["source"]), relative_path(row["record"])
        if PurePosixPath(source).name != name.removeprefix("bin/"):
            raise PackageError("selected_tool_name_mismatch")
        group = "tool_updates" if name in donor["tools"] else "tool_additions"
        donor.setdefault(group, {})[name] = {"source": source, "sha256": row["sha256"], "record": record}
    settings["tool_record_pins"] = {name: row["record_sha256"] for name, row in tools.items()}
    settings["input_selection"] = {"path": str(path), "sha256": expected, "bytes": len(raw)}
    settings["input_selection_raw"] = raw
    return settings



def selected_consumer_closure(inputs: Inputs, files: dict, selection: dict, target: str) -> bytes:
    """Join the existing export to its retained dependency record, not a target label."""
    path, expected = selection["dependency_closure"], selection["dependency_closure_sha256"]
    raw = inputs.read(path)
    if digest(raw) != expected:
        raise PackageError("selected_consumer_closure_digest_mismatch")
    record = json.loads(raw, object_pairs_hook=unique_object)
    manifest = json.loads(files["CONTENTS.json"][0], object_pairs_hook=unique_object)
    accepted = manifest.get("accepted_inputs")
    if not isinstance(accepted, dict) or accepted.get(path) != expected:
        raise PackageError("selected_consumer_closure_binding_mismatch")
    platform = record.get("python_platform") if isinstance(record, dict) else None
    system, machine = {"macos-arm64": ("Darwin", "arm64"), "linux-x86_64": ("Linux", "x86_64")}[target]
    if (not isinstance(record, dict) or type(record.get("schema_version")) is not int
            or record["schema_version"] != 1
            or record.get("scope") != "dependency_bytes_for_existing_consumer_selection"
            or not isinstance(platform, dict) or platform.get("implementation") != "cpython"
            or platform.get("version") != "3.11" or platform.get("system") != system
            or platform.get("machine") != machine):
        raise PackageError("selected_consumer_target_mismatch")
    required = {"agno/runtime-constraints.txt", "services/runtime-constraints.txt",
                "activepieces/package.json", "activepieces/package-lock.json"}
    for prefix, suffix in (("agno/meerkat_toolkit_agno-", ".whl"),
                           ("services/meerkat_toolkit_services-", ".whl"),
                           ("activepieces/rkat-toolkit-activepieces-", ".tgz")):
        selected = [name for name in files if name.startswith(prefix) and name.endswith(suffix)]
        if len(selected) != 1:
            raise PackageError("selected_consumer_component_mismatch")
        required.add(selected[0])
    bindings = record.get("accepted_inputs")
    if (not isinstance(bindings, dict) or set(bindings) != required
            or any(name not in files or bindings[name] != digest(files[name][0]) for name in required)):
        raise PackageError("selected_consumer_component_mismatch")
    version = manifest.get("schema_version")
    runtime = manifest.get("runtime_target")
    if (type(version) is not int or manifest.get("scope") != "consumer_export_only_no_installation_or_activation"
            or (target == "macos-arm64" and (version != 1 or "runtime_target" in manifest))
            or (target == "linux-x86_64" and (version != 2 or not isinstance(runtime, dict)
                or not isinstance(runtime.get("python"), dict) or not isinstance(runtime.get("node"), dict)
                or runtime["python"].get("platform") != "linux" or runtime["python"].get("machine") != machine
                or type(runtime["python"].get("pointer_bits")) is not int or runtime["python"]["pointer_bits"] != 64
                or runtime["node"].get("platform") != "linux" or runtime["node"].get("arch") != "x64"))):
        raise PackageError("selected_consumer_target_mismatch")
    return raw


def validate_selected_tool_record(name: str, target: str, selected: dict,
                                  data: bytes, raw: bytes, expected: str) -> None:
    if digest(raw) != expected:
        raise PackageError("selected_tool_build_record_digest_mismatch")
    record = json.loads(raw, object_pairs_hook=unique_object)
    triple = {"macos-arm64": "aarch64-apple-darwin", "linux-x86_64": "x86_64-unknown-linux-gnu"}[target]
    if (not isinstance(record, dict) or record.get("binary") != selected["source"]
            or record.get("sha256") != selected["sha256"] or record.get("sha256") != digest(data)
            or type(record.get("bytes")) is not int or record["bytes"] != len(data)
            or record.get("mode") != "0755" or record.get("target") != triple):
        raise PackageError("selected_tool_build_record_mismatch")
    command = record.get("command")
    if (not isinstance(command, list) or not all(isinstance(arg, str) for arg in command)
            or command.count("--bin") != 1 or command.index("--bin") + 1 >= len(command)
            or command[command.index("--bin") + 1] != name.removeprefix("bin/")):
        raise PackageError("selected_tool_build_command_mismatch")
    if name == "bin/toolkit-local-recipes" and (not isinstance(record.get("features"), list)
            or "local-recipes" not in record["features"]):
        raise PackageError("selected_tool_build_features_mismatch")


def parent_directories(names: set[str]) -> set[str]:
    return {str(parent) for name in names for parent in PurePosixPath(name).parents if str(parent) != "."}


def collect_bundle(inputs: Inputs, selection: dict, *, official: bool = False) -> dict[str, tuple[bytes, int]]:
    path = selection["bundle"]
    contents = inputs.read(path + "/CONTENTS.json")
    sums = inputs.read(path + "/SHA256SUMS")
    if digest(contents) != selection["contents"] or digest(sums) != selection["sums"]:
        raise PackageError("selected_manifest_digest_mismatch")
    document = json.loads(contents, object_pairs_hook=unique_object)
    rows = document.get("files") if isinstance(document, dict) else None
    if not isinstance(rows, dict) or not rows or len(rows) > MAX_ENTRIES:
        raise PackageError("invalid_contents")
    expected = {"CONTENTS.json": digest(contents)}
    for name, row in rows.items():
        relative_path(name)
        if name in {"CONTENTS.json", "SHA256SUMS"} or not isinstance(row, dict) or not isinstance(row.get("sha256"), str) or not HEX.fullmatch(row["sha256"]):
            raise PackageError("invalid_contents")
        expected[name] = row["sha256"]
    actual_sums = {}
    if not sums.endswith(b"\n"):
        raise PackageError("invalid_checksums")
    for line in sums.decode("ascii").splitlines():
        checksum, separator, name = line.partition("  ")
        relative_path(name)
        if separator != "  " or not HEX.fullmatch(checksum) or name in actual_sums:
            raise PackageError("invalid_checksums")
        actual_sums[name] = checksum
    if actual_sums != expected:
        raise PackageError("checksums_disagree")
    names = set(expected) | {"SHA256SUMS"}
    files, directories = inputs.file_set(path)
    if files != names or directories != parent_directories(names):
        raise PackageError("bundle_file_set_mismatch")
    result = {"CONTENTS.json": (contents, 0o644), "SHA256SUMS": (sums, 0o644)}
    for name, row in rows.items():
        executable = official and name == OFFICIAL_EXECUTABLE
        data = inputs.read(path + "/" + name, executable=executable)
        if digest(data) != row["sha256"] or ("bytes" in row and (type(row["bytes"]) is not int or row["bytes"] != len(data))):
            raise PackageError("selected_payload_digest_mismatch")
        result[name] = (data, 0o755 if executable else 0o644)
    if official and OFFICIAL_EXECUTABLE not in result:
        raise PackageError("missing_official_executable")
    return result


def collect_official(inputs: Inputs, selection: dict, target: str) -> dict[str, tuple[bytes, int]]:
    """Bind a selected official payload to this archive's target and components.

    This checks packaging consistency only. The installer owns executable and
    configuration admission, and runtime evidence remains with each component.
    """
    files = collect_bundle(inputs, selection, official=True)
    if "delivery.json" not in files:
        raise PackageError("missing_official_delivery")
    delivery = json.loads(files["delivery.json"][0], object_pairs_hook=unique_object)
    if not isinstance(delivery, dict) or delivery.get("format") != "meerkat-toolkit.official-consumers.v1":
        raise PackageError("invalid_official_delivery")
    expected = list(selection["components"])
    components = delivery.get("components")
    if (not isinstance(components, list) or not all(isinstance(row, dict) for row in components)
            or delivery.get("catalog_ids") != expected
            or [row.get("catalog_id") for row in components] != expected):
        raise PackageError("official_components_mismatch")
    # Omitted machine targets are the historical macOS contract only. Never
    # infer a Linux target from runtime-profile prose or the packaging host.
    if any(row.get("target", "macos-arm64") != target for row in components):
        raise PackageError("official_target_mismatch")
    if any(name not in {"README.md", "delivery.json", "CONTENTS.json", "SHA256SUMS"}
           and name.split("/", 1)[0] not in expected for name in files):
        raise PackageError("official_components_mismatch")
    return files


def compose_official_routes(files: dict[str, tuple[bytes, int]], bindings: list[dict]) -> None:
    """Bind explicit official choices at the owner of both delivery payloads.

    Inner exports remain unchanged. Only this release's routes and their two
    manifest records are replaced; catalog identities and all artifacts stay exact.
    """
    if not bindings:
        return
    def document(path):
        return json.loads(files[path][0], object_pairs_hook=unique_object)
    def encoded(value):
        return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    try:
        index = document("consumers/routes.json")
        catalog = document("consumers/catalog.json")
        manifest = document("consumers/CONTENTS.json")
        official = document("official/CONTENTS.json")
        delivery = document("official/delivery.json")
        original_sha = digest(files["consumers/routes.json"][0])
        if (type(index["schema_version"]) is not int or index["schema_version"] != 1
                or index["catalog_sha256"] != digest(files["consumers/catalog.json"][0])
                or manifest["files"]["routes.json"]["sha256"] != original_sha):
            raise PackageError("invalid_official_route_index")
        contents_sha = digest(files["official/CONTENTS.json"][0])
        installer_sha = digest(files["setup/install-library-consumer.py"][0])
        supported = {"official-github": ("github", "selected-reads", "github/github-mcp-server", "artifact"),
                     "official-playwright": ("playwright", "isolated-chrome", "playwright/package-lock.json", "package_lock")}
        for binding in bindings:
            if not isinstance(binding, dict) or set(binding) != {"identity", "strategy_id", "family", "profile", "path"}:
                raise PackageError("invalid_official_route_binding")
            family = binding["family"]
            name, profile, path, delivery_key = supported[family]
            identity = binding["identity"]
            if (not isinstance(identity, dict) or set(identity) != {"publisher", "name", "version"}
                    or identity["publisher"] != "meerkat-toolkit" or identity["name"] != name
                    or not isinstance(identity["version"], str) or not identity["version"].startswith("metadata-sha256:")
                    or not HEX.fullmatch(identity["version"].removeprefix("metadata-sha256:"))
                    or binding["strategy_id"] != "selected-local-mcp"
                    or binding["profile"] != profile or binding["path"] != path
                    or family in index["components"]):
                raise PackageError("invalid_official_route_binding")
            entries = [entry for entry in catalog if entry["identity"] == identity]
            strategies = [strategy for entry in entries for strategy in entry["strategies"]
                          if strategy["id"] == binding["strategy_id"]]
            declarations = [row for row in delivery["components"] if row["catalog_id"] == name]
            if (len(entries) != 1 or len(strategies) != 1 or strategies[0]["kind"] != "local_mcp"
                    or len(declarations) != 1 or declarations[0][delivery_key] != path
                    or any(row["identity"] == identity and row["strategy_id"] == binding["strategy_id"] for row in index["routes"])):
                raise PackageError("official_route_binding_mismatch")
            artifact_sha = digest(files["official/" + path][0])
            if official["files"][path]["sha256"] != artifact_sha:
                raise PackageError("official_route_artifact_mismatch")
            index["components"][family] = {"path": path, "sha256": artifact_sha, "profiles": [profile],
                                            "contents_sha256": contents_sha, "installer_sha256": installer_sha}
            index["routes"].append({key: binding[key] for key in ("identity", "strategy_id", "family", "profile")})
        index["schema_version"] = 2
        raw = encoded(index)
        manifest["files"]["routes.json"] = {"sha256": digest(raw), "source": {
            "kind": "release_composed_official_routes", "prior_routes_sha256": original_sha,
            "official_contents_sha256": contents_sha, "official_installer_sha256": installer_sha}}
        contents = encoded(manifest)
        checksums = {name: row["sha256"] for name, row in manifest["files"].items()}
        checksums["CONTENTS.json"] = digest(contents)
        sums = "".join(checksum + "  " + name + "\n" for name, checksum in sorted(checksums.items())).encode()
    except (KeyError, TypeError, ValueError):
        raise PackageError("invalid_official_route_binding") from None
    files.update({"consumers/routes.json": (raw, 0o644), "consumers/CONTENTS.json": (contents, 0o644),
                  "consumers/SHA256SUMS": (sums, 0o644)})


def collect_python(inputs: Inputs, target: str, *, settings: dict | None = None) -> dict[str, tuple[bytes, int]]:
    """Select accepted Python tools from an exact donor, without extracting it.

    The whole archive pin identifies the prior accepted distribution. Its
    original provenance stays data; this is not another provenance authority.
    """
    donor = settings["donor"] if settings is not None else PYTHON_KITS[target]
    record_pins = settings.get("tool_record_pins", {}) if settings is not None else {}
    if donor["root"] != "agent-install-kit-" + target:
        raise PackageError("python_donor_target_mismatch")
    raw = inputs.read(donor["archive"])
    if digest(raw) != donor["sha256"]:
        raise PackageError("python_donor_digest_mismatch")
    trees = ("licenses/crates/", "licenses/rust/", "licenses/toolkit/",
             "provenance/crates/", *donor["provenance_trees"])
    contracts = PYTHON_DONOR_CONTRACTS | donor.get("contracts", {})
    exact = {*donor["tools"], *donor["provenance_files"], "provenance/dependencies.json",
             "provenance/kit.json", *(name.removeprefix("selected/") for name in contracts)}
    result, selected, seen = {}, {}, set()
    expanded = 0
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as archive:
        for member in archive:
            name = member.name.removeprefix(donor["root"] + "/")
            if (member.name != donor["root"] + "/" + name or name in seen
                    or not member.isfile() or member.mode not in (0o644, 0o755)):
                raise PackageError("invalid_python_donor_member")
            relative_path(name)
            seen.add(name)
            expanded += member.size
            if (len(seen) > MAX_DONOR_ENTRIES or member.size < 0
                    or member.size > MAX_FILE_BYTES or expanded > MAX_TOTAL_BYTES):
                raise PackageError("python_donor_too_large")
            if name not in exact and not name.startswith(trees):
                continue
            with archive.extractfile(member) as stream:
                data = stream.read(MAX_FILE_BYTES + 1)
            if len(data) != member.size:
                raise PackageError("invalid_python_donor_member")
            if name in donor["tools"]:
                if digest(data) != donor["tools"][name] or member.mode != 0o755:
                    raise PackageError("python_tool_digest_mismatch")
            elif member.mode != 0o644:
                raise PackageError("invalid_python_donor_member")
            # Name the whole-kit record as historical donor provenance. It
            # deliberately describes more capabilities than this selection.
            output = "provenance/donor-kit.json" if name == "provenance/kit.json" else name
            result["selected/" + output] = (data, member.mode)
            selected[name] = digest(data)
    if not exact <= seen or any(not any(name.startswith(tree) for name in selected) for tree in trees):
        raise PackageError("incomplete_python_donor")
    updates = {}
    for name, update in donor.get("tool_updates", {}).items():
        if name not in donor["tools"]:
            raise PackageError("unknown_selected_tool_update")
        data = inputs.read(update["source"], executable=True)
        record = inputs.read(update["record"])
        if digest(data) != update["sha256"]:
            raise PackageError("selected_tool_update_digest_mismatch")
        if name in record_pins:
            validate_selected_tool_record(name, target, update, data, record, record_pins[name])
        try:
            if json.loads(record)["sha256"] != update["sha256"]:
                raise ValueError()
        except (KeyError, TypeError, ValueError):
            raise PackageError("selected_tool_update_record_mismatch") from None
        result["selected/" + name] = (data, 0o755)
        result["selected/provenance/updates/" + name.removeprefix("bin/") + ".json"] = (record, 0o644)
        updates[name] = {"source": update["source"], "sha256": update["sha256"],
                         "record_sha256": digest(record), "previous_sha256": selected[name]}
        selected[name] = update["sha256"]
    additions = {}
    declared = donor.get("tool_additions", {})
    if not isinstance(declared, dict):
        raise PackageError("invalid_selected_tool_addition")
    occupied = {*result, *SOURCES, *PYTHON_SOURCES, *donor.get("sources", {}),
                *TARGETS[target].get("setup_assets", {}), *("selected/" + path for path in seen)}
    for name, addition in declared.items():
        if (not isinstance(name, str) or not name.startswith("bin/") or len(name.split("/")) != 2
                or not isinstance(addition, dict) or set(addition) != {"source", "sha256", "record"}
                or not isinstance(addition["sha256"], str) or not HEX.fullmatch(addition["sha256"])):
            raise PackageError("invalid_selected_tool_addition")
        output = "selected/" + relative_path(name)
        provenance = "selected/provenance/additions/" + name.removeprefix("bin/") + ".json"
        for candidate in (output, provenance):
            if any(candidate.casefold() == path.casefold()
                   or candidate.casefold().startswith(path.casefold() + "/")
                   or path.casefold().startswith(candidate.casefold() + "/") for path in occupied):
                raise PackageError("selected_tool_addition_collision")
        data = inputs.read(addition["source"], executable=True)
        record = inputs.read(addition["record"])
        if digest(data) != addition["sha256"]:
            raise PackageError("selected_tool_addition_digest_mismatch")
        if name in record_pins:
            validate_selected_tool_record(name, target, addition, data, record, record_pins[name])
        try:
            binding = json.loads(record, object_pairs_hook=unique_object)
            if binding["sha256"] != addition["sha256"] or binding["binary"] != addition["source"]:
                raise ValueError()
        except (KeyError, TypeError, ValueError):
            raise PackageError("selected_tool_addition_record_mismatch") from None
        result[output] = (data, 0o755)
        result[provenance] = (record, 0o644)
        occupied.update((output, provenance))
        additions[name] = {"source": addition["source"], "sha256": addition["sha256"],
                           "record_sha256": digest(record)}
        selected[name] = addition["sha256"]
    source_updates = {}
    installer_source = "selected/bin/install-library-consumer.py"
    selected_sources = settings.get("sources", {}) if settings is not None else {}
    if installer_source in selected_sources:
        name = installer_source.removeprefix("selected/")
        if name not in donor["tools"] or name in updates or name in additions:
            raise PackageError("duplicate_selected_installer_source")
        path, expected = selected_sources[installer_source]
        data = inputs.read(path)
        if digest(data) != expected:
            raise PackageError("selected_source_digest_mismatch")
        result[installer_source] = (data, 0o755)
        source_updates[name] = {"path": path, "sha256": expected, "previous_sha256": selected[name]}
        selected[name] = expected
    record = {"scope": "retained_selected_tools", "platform": target,
              "donor": {"path": donor["archive"], "sha256": donor["sha256"]}, "files": selected}
    if source_updates:
        record["source_updates"] = source_updates
    if updates:
        record["tool_updates"] = updates
    if additions:
        record["tool_additions"] = additions
    if updates or additions:
        record["capabilities"] = donor.get("capabilities", [])
    result["selected/provenance/selection.json"] = ((json.dumps(record, indent=2, sort_keys=True) + "\n").encode(), 0o644)
    return result


def python_document(source: str, destination: str, raw: bytes, *, target: str | None = None,
                    settings: dict | None = None) -> bytes:
    """Relocate shipped links; label unshipped documents as repository paths."""
    python_sources = settings["python_sources"] if settings is not None else PYTHON_SOURCES
    destinations = {path: name for name, (path, _) in python_sources.items()}
    destinations.update({path: name for name, path in PYTHON_DONOR_CONTRACTS.items()})
    if target is not None:
        donor = settings["donor"] if settings is not None else PYTHON_KITS[target]
        if settings is None:
            destinations.update({path: name for name, (path, _) in donor.get("sources", {}).items()})
        destinations.update({path: name for name, path in donor.get("contracts", {}).items()})

    def replace(match):
        label, url = match.groups()
        if ":" in url or url.startswith("#"):
            return match.group()
        # This retained donor link names its kit root, not the original
        # repository's crate parent. Point it at this delivery's host guide.
        if source == "crates/toolkit-package/LOCAL-RECIPES.md" and url == "../README.md":
            return "[" + label + "](" + posixpath.relpath("README.md", posixpath.dirname(destination)) + ")"
        path, separator, anchor = url.partition("#")
        path = posixpath.normpath(posixpath.join(posixpath.dirname(source), path))
        if path.startswith("../") or path.startswith("/"):
            raise PackageError("invalid_contract_link")
        suffix = separator + anchor if separator else ""
        if path not in destinations:
            return label + " (source repository path: `" + path + suffix + "`; not included)"
        link = posixpath.relpath(destinations[path], posixpath.dirname(destination))
        return "[" + label + "](" + link + suffix + ")"

    return re.sub(r"\[([^\]]*)\]\(([^)]+)\)", replace, raw.decode("utf-8")).encode("utf-8")


def collect(root: Path, target: str, *, settings: dict | None = None) -> dict[str, tuple[bytes, int]]:
    if target not in TARGETS:
        raise PackageError("unsupported_target")
    selection = settings["selection"] if settings is not None else TARGETS[target]
    donor = settings["donor"] if settings is not None else PYTHON_KITS[target]
    python_sources = settings["python_sources"] if settings is not None else PYTHON_SOURCES | donor.get("sources", {})
    inputs = Inputs(root)
    consumer = collect_bundle(inputs, selection)
    result = {"consumers/" + name: value for name, value in consumer.items()}
    if settings is not None and "input_selection" in settings:
        result["selected/provenance/consumer-dependency-closure.json"] = (
            selected_consumer_closure(inputs, consumer, selection, target), 0o644)
    if selection.get("official") is not None:
        result.update({"official/" + name: value for name, value in collect_official(inputs, selection["official"], target).items()})
    result.update(collect_python(inputs, target, settings=settings))
    sources = settings["sources"] if settings is not None else setup_sources(selection)
    for name, (path, expected) in sources.items():
        if name == "selected/bin/install-library-consumer.py":
            continue  # Already checked and recorded with the retained tools.
        data = inputs.read(path)
        if digest(data) != expected:
            raise PackageError("selected_source_digest_mismatch")
        result[relative_path(name)] = (data, 0o644)
    document_sources = {}
    for name, path in (PYTHON_DONOR_CONTRACTS | donor.get("contracts", {})).items():
        original, mode = result[name]
        # Retain the executable's contemporaneous contract, relocating only
        # the old kit's evidence links back to explicit repository references.
        data = python_document(path, name, original.replace(b"../provenance/references/", b"../../"), target=target, settings=settings)
        result[name] = (data, mode)
        document_sources[name] = {"donor_member": name.removeprefix("selected/"),
                                  "source_sha256": digest(original), "output_sha256": digest(data)}
    for name, (path, expected) in python_sources.items():
        data = inputs.read(path)
        if digest(data) != expected:
            raise PackageError("selected_source_digest_mismatch")
        if name.endswith(".md") and (name.startswith("selected/contracts/")
                                    or name == "selected/agent/instructions.md"):
            original = data
            data = python_document(path, name, data, target=target, settings=settings)
            document_sources[name] = {"source": path, "source_sha256": digest(original),
                                      "output_sha256": digest(data)}
        result[relative_path(name)] = (data, 0o644)
    document_record = {"base_source_revision": SOURCE_REVISION, "files": document_sources}
    result["selected/provenance/document-sources.json"] = ((json.dumps(document_record, indent=2, sort_keys=True) + "\n").encode(), 0o644)
    if settings is not None and "input_selection_raw" in settings:
        # Exact producer submission, not a replacement for historical records or
        # a claim that these inputs have completed execution qualification.
        result["selected/provenance/current-input-selection.json"] = (settings["input_selection_raw"], 0o644)
    # Static serving is fixed to this directory. Do not silently ship new assets.
    static_files, static_dirs = inputs.file_set("sdk/consumer-setup/static")
    if static_files != {"index.html", "app.js", "form.js", "style.css"} or static_dirs:
        raise PackageError("setup_file_set_mismatch")
    compose_official_routes(result, OFFICIAL_ROUTE_BINDINGS[target])
    # Raw reads, donor expansion and final payload have separate bounded totals.
    if (any(len(data) > MAX_FILE_BYTES for data, _ in result.values())
            or sum(len(data) for data, _ in result.values()) > MAX_TOTAL_BYTES):
        raise PackageError("output_too_large")
    validate_guide_commands(result["README.md"][0], result)
    return result


def validate_guide_commands(guide: bytes, files: dict) -> None:
    """Keep literal bootstrap pins bound to the payloads being packaged."""
    text = guide.decode("utf-8")
    for flag, name in (("--contents-sha256", "consumers/CONTENTS.json"),
                       ("--official-contents-sha256", "official/CONTENTS.json")):
        pattern = re.escape(flag) + r"(?:[\"']\s*,\s*[\"']|\s+)([0-9a-f]{64})"
        pins = re.findall(pattern, text)
        if pins and (name not in files or any(pin != digest(files[name][0]) for pin in pins)):
            raise PackageError("guide_command_digest_mismatch")


def package(root: Path, target: str, output: Path, *, input_selection: Path | None = None,
            input_selection_sha256: str | None = None) -> dict:
    if target not in TARGETS or not output.is_absolute() or output.name in {"", ".", ".."}:
        raise PackageError("invalid_output_or_target")
    if output.exists() or output.is_symlink():
        raise PackageError("output_exists")
    # A trusted local parent is required. Canonicalizing permits /tmp on macOS.
    output = output.parent.resolve(strict=True) / output.name
    root = root.resolve(strict=True)
    settings = selected_inputs(root, target, input_selection, input_selection_sha256)
    source_sha256 = digest(Path(__file__).read_bytes()) if input_selection is not None else None
    protected = [selection["bundle"] for selection in TARGETS.values()]
    protected.extend(selection["official"]["bundle"] for selection in TARGETS.values()
                     if selection.get("official") is not None)
    protected.extend((settings["selection"]["bundle"], "sdk/consumer-setup/static"))
    if any(output.is_relative_to(root / path) for path in protected):
        raise PackageError("output_inside_input")
    files = collect(root, target, settings=settings if input_selection is not None else None)
    edition = settings["selection"].get("edition", EDITION)
    root_name = f"toolkit-library-{edition}-{target}"
    archive_name = root_name + ".tar.gz"
    os.mkdir(output, 0o700)
    temporary = output / (archive_name + ".part")
    with temporary.open("xb") as stream:
        with gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.PAX_FORMAT) as archive:
                members = {root_name + "/" + name: value for name, value in files.items()}
                directories = parent_directories(set(members))
                for name in sorted(set(members) | directories):
                    info = tarfile.TarInfo(name)
                    info.uid = info.gid = info.mtime = 0
                    info.uname = info.gname = ""
                    if name in directories:
                        info.type = tarfile.DIRTYPE
                        info.mode = 0o755
                        archive.addfile(info)
                    else:
                        data, info.mode = members[name]
                        info.size = len(data)
                        archive.addfile(info, io.BytesIO(data))
    destination = output / archive_name
    temporary.rename(destination)
    with destination.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    with (output / "SHA256SUMS").open("x", encoding="ascii", newline="\n") as stream:
        stream.write(f"{checksum}  {archive_name}\n")
    result = {"target": target, "archive": str(destination), "sha256": checksum, "files": len(files)}
    if input_selection is not None:
        if digest(Path(__file__).read_bytes()) != source_sha256:
            raise PackageError("packager_source_changed")
        result.update({"schema_version": 1, "archive_bytes": destination.stat().st_size,
                       "packager_source_sha256": source_sha256,
                       "input_selection": settings["input_selection"],
                       "scope": "explicit_selected_library_packaging_not_execution_qualification"})
        with (output / "build.json").open("x", encoding="utf-8") as stream:
            stream.write(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--target", choices=tuple(TARGETS), required=True)
    parser.add_argument("--output", type=Path, required=True, help="New absolute output directory; its parent must exist")
    parser.add_argument("--input-selection", type=Path)
    parser.add_argument("--input-selection-sha256")
    args = parser.parse_args()
    try:
        result = package(ROOT, args.target, args.output, input_selection=args.input_selection,
                         input_selection_sha256=args.input_selection_sha256)
    except PackageError as error:
        parser.exit(1, f"Packaging refused: {error}. Inputs and earlier outputs were not replaced.\n")
    except (OSError, ValueError, TypeError, UnicodeError, RecursionError):
        parser.exit(1, "Packaging refused: input_or_output_unavailable. Inputs and earlier outputs were not replaced.\n")
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
