import hashlib
import hmac
import json
from pathlib import Path
import shutil

import pytest

import launchkit.dom_check as dom_check
import launchkit.implementation_workspace as execution


FIXTURES = Path(__file__).parent / "fixtures" / "dom_drafts"
TEST_KEY = b"synthetic-public-completion-key!!"


def signed_completion(value):
    payload = json.dumps(value, separators=(",", ":"))
    return dom_check.DOM_RESULT_MARKER + hmac.digest(TEST_KEY, payload.encode(), "sha256").hex() + "=" + payload + "\n"


@pytest.fixture
def run_dom(tmp_path):
    if not execution._sandbox_executable() or not shutil.which("node"):
        pytest.skip("DOM execution requires the trusted Node runtime and OS sandbox.")
    count = 0

    def run(content, *, html=None, timeout=5, host_input_override=None):
        nonlocal count
        count += 1
        staging = tmp_path / str(count)
        staging.mkdir()
        source, protected, scratch = [staging / name for name in ("source", "tests", "scratch")]
        for folder in (source, protected, scratch):
            folder.mkdir()
        if html is not None:
            (source / "draft.html").write_text(html)
        (protected / "test_dom.dom.cjs").write_text(content)

        def execute(command, reads, input_bytes=None):
            if input_bytes is not None and host_input_override is not None:
                input_bytes = host_input_override
            return execution._run_sandboxed(command, execution._sandbox_executable(),
                execution._sandbox_profile(source, scratch, staging, protected_reads=reads),
                source, execution._check_environment(scratch), timeout=timeout,
                **({"input_bytes": input_bytes} if input_bytes is not None else {}))

        result = dom_check.run_dom_tests(["test_dom.dom.cjs"], source, protected, scratch, staging, execute)
        return result, source, protected, scratch, staging

    return run


# These tests run the exact historical production inline script, not a rewritten
# acknowledgment algorithm. CRLF is supplied explicitly as a server observation;
# JSDOM does not prove that a browser encodes a form or navigates correctly.
DRAFT_TEST = r'''const fs = require("node:fs");
const path = require("node:path");
module.exports = ({test, createDOM, assert, sourceRoot}) => {
  const script = fs.readFileSync(path.join(sourceRoot, "draft.html"), "utf8");
  const key = "seestar:draft:v1:plan:new";
  const fields = {setup:"selected-setup-v3", date:"2026-10-01", site:"  Skog 🌌  ", minutes:"075",
    forecast:"first line\nsecond line", source:"  declared evidence  ", issued:"2026-10-01T21:00:00Z",
    visibility:" clear\n limited ", operation:"operation-A"};
  const html = acknowledgments => `<script id="draft-acknowledgements" type="application/json">${JSON.stringify(acknowledgments)}</script>
    <form data-draft="plan:new">${Object.keys(fields).map(name =>
      ["forecast","visibility"].includes(name) ? `<textarea name="${name}"></textarea>` : `<input name="${name}">`).join("")}</form>${script}`;
  const load = (ack = [], stored) => createDOM(html(ack), {url:"https://offline.invalid/",
    beforeParse(window) { if (stored) window.localStorage.setItem(key, stored); }});
  const edit = (dom, name, value) => { const element=dom.window.document.querySelector(`[name="${name}"]`);
    element.value=value; element.dispatchEvent(new dom.window.Event("input", {bubbles:true})); };
  const submitA = dom => {
    for (const [name,value] of Object.entries(fields)) edit(dom,name,value);
    dom.window.document.querySelector("form").dispatchEvent(new dom.window.Event("submit", {cancelable:true,bubbles:true}));
    return JSON.parse(dom.window.localStorage.getItem(key));
  };
  const acknowledge = saved => ({scope:"plan:new", edit:saved.edit, values:Object.fromEntries(
    Object.entries(saved.values).map(([name,value]) => [name,["forecast","visibility"].includes(name) ? value.replace(/\n/g,"\r\n") : value]))});
  test("textarea server newline acknowledgment retains raw tokens and clears exact submitted A", () => {
    const first=load(), saved=submitA(first);
    assert.equal(saved.values.minutes,"075"); assert.equal(saved.values.forecast,fields.forecast);
    assert.equal(saved.values.site,fields.site); assert.equal(saved.submitted,true);
    const readback=load([acknowledge(saved)],JSON.stringify(saved));
    assert.equal(readback.window.localStorage.getItem(key),null);
  });
  test("actual DOM edit B survives the older submitted A acknowledgment", () => {
    const first=load(), savedA=submitA(first);
    edit(first,"forecast","newer B\nwith whitespace  ");
    const storedB=first.window.localStorage.getItem(key), savedB=JSON.parse(storedB);
    assert.notEqual(savedB.edit,savedA.edit); assert.notEqual(savedB.values.operation,savedA.values.operation);
    assert.equal(savedB.submitted,false);
    const readback=load([acknowledge(savedA)],storedB);
    assert.equal(readback.window.localStorage.getItem(key),storedB);
    assert.equal(readback.window.document.querySelector('[name="forecast"]').value,savedB.values.forecast);
  });
};
'''


@pytest.mark.parametrize("variant,expected", [("original", "fail"), ("repaired", "pass")])
def test_exact_production_script_detects_newline_ack_and_preserves_newer_edit(run_dom, variant, expected):
    result, *_ = run_dom(DRAFT_TEST, html=(FIXTURES / f"{variant}.html").read_text())
    assert result["status"] == expected, result
    assert result["tests_run"] == 2
    assert result["test_results"][1]["status"] == "pass"
    assert "not browser form encoding" in result["detail"]


def test_exact_production_script_provenance():
    provenance = json.loads((FIXTURES / "provenance.json").read_text())
    for variant in ("original", "repaired"):
        assert hashlib.sha256((FIXTURES / f"{variant}.html").read_bytes()).hexdigest() == provenance[variant]["script_sha256"]


def test_sandbox_denies_host_files_network_fork_and_source_test_dependency_writes(run_dom, tmp_path, monkeypatch):
    secret = tmp_path / "private-outside.txt"
    secret.write_text("not-readable")
    monkeypatch.setenv("DOM_HOST_SECRET", "not-inherited")
    source = r'''const fs = require("node:fs"), path=require("node:path"), net=require("node:net");
module.exports=({test,assert,sourceRoot,scratchRoot}) => {
  const denied = fn => assert.throws(fn, error => ["EPERM","EACCES","ERR_ACCESS_DENIED"].includes(error.code));
  test("OS denies payload reads and all nonscratch writes", () => {
    assert.equal(process.env.DOM_HOST_SECRET,undefined);
    denied(() => fs.readFileSync(__OUTSIDE_PATH__));
    denied(() => fs.writeFileSync(path.join(sourceRoot,"draft.html"),"changed"));
    denied(() => fs.writeFileSync(__filename,"changed"));
    denied(() => fs.writeFileSync(path.join(process.argv[5],"node_modules/jsdom/package.json"),"changed"));
    fs.writeFileSync(path.join(scratchRoot,"allowed.txt"),"scratch-only");
    assert.equal(fs.readFileSync(path.join(scratchRoot,"allowed.txt"),"utf8"),"scratch-only");
  });
  test("OS denies outbound sockets", async () => {
    const error=await new Promise((resolve,reject) => {
      const socket=net.connect({host:"127.0.0.1",port:9});
      socket.on("connect",()=>{socket.destroy();reject(new Error("Network was allowed"));});
      socket.on("error",resolve); socket.setTimeout(500,()=>{socket.destroy();reject(new Error("Network denial not observed"));});
    }).catch(error=>error);
    assert.ok(["EPERM","EACCES","ERR_ACCESS_DENIED"].includes(error.code),error.code);
  });
  test("OS denies child process creation", () => {
    denied(()=>require("node:child_process").spawnSync(process.execPath,["-e","process.exit(0)"]));
  });
};'''.replace("__OUTSIDE_PATH__", json.dumps(str(secret)))
    result, source_root, protected, scratch, staging = run_dom(source, html="unchanged")
    assert result["status"] == "pass", result
    assert result["tests_run"] == 3
    assert (source_root / "draft.html").read_text() == "unchanged"
    assert (protected / "test_dom.dom.cjs").read_text() == source
    assert (staging / "dom-dependencies/node_modules/jsdom/package.json").read_bytes() != b"changed"
    assert (scratch / "allowed.txt").read_text() == "scratch-only"
    assert secret.read_text() == "not-readable"


def test_private_checker_path_runs_production_dom_and_preserves_os_denials(run_dom, tmp_path, monkeypatch):
    import os
    from launchkit import checker_runtime
    # Explicit preparation is separate from checks. A developer can select an
    # owned prepared cache for this offline test without touching the installer's
    # tools. Copying a package-manager Node can break its shared-library paths.
    root = Path(os.environ.get("MOBKIT_TEST_CHECKER_ROOT", Path(__file__).resolve().parents[1]))
    if checker_runtime.prepared_node(root) is None:
        pytest.skip("Explicitly prepare a private checker to test its executable path; no test downloads it.")
    monkeypatch.setattr(dom_check, "resolve_node", lambda configured: checker_runtime.resolve_node(configured, root=root))
    secret = tmp_path / "outside-private-checker.txt"
    secret.write_text("host-only")
    test = r'''const fs=require("node:fs"),path=require("node:path");
module.exports=({test,createDOM,assert,sourceRoot,scratchRoot})=>{
 test("actual production click",()=>{
  const dom=createDOM(fs.readFileSync(path.join(sourceRoot,"draft.html"),"utf8"));
  dom.window.document.querySelector("button").click();
  assert.equal(dom.window.document.querySelector("output").textContent,"1");
 });
 test("private runtime grants do not expose host files or source writes",()=>{
  assert.throws(()=>fs.readFileSync(__OUTSIDE__),e=>["EPERM","EACCES","ERR_ACCESS_DENIED"].includes(e.code));
  assert.throws(()=>fs.writeFileSync(path.join(sourceRoot,"draft.html"),"changed"),e=>["EPERM","EACCES","ERR_ACCESS_DENIED"].includes(e.code));
  fs.writeFileSync(path.join(scratchRoot,"allowed.txt"),"scratch");
 });
};'''.replace("__OUTSIDE__", json.dumps(str(secret)))
    result, *_ = run_dom(test, html='<button>Add</button><output>0</output><script>document.querySelector("button").onclick=()=>document.querySelector("output").textContent="1";</script>')
    assert result["status"] == "pass" and result["tests_run"] == 2, result
    assert secret.read_text() == "host-only"


@pytest.mark.parametrize("content", [
    "module.exports=()=>{};",
    "module.exports=({test})=>test('pending',()=>{}, {skip:true});",
    "process.exit(0);",
    "console.log('MOBKIT_DOM_RESULT=fixed={\"tests_run\":1}');process.exit(0);",
    "module.exports=({test})=>{test('same',()=>{});test('same',()=>{});};",
    "module.exports=({test,createDOM})=>test('script',()=>createDOM('<script>throw new Error(\"failed script\")</script>'));",
])
def test_empty_skipped_premature_or_script_failed_execution_never_passes(run_dom, content):
    result, *_ = run_dom(content)
    assert result["status"] == "fail", result


@pytest.mark.parametrize("content", [
    "module.exports=({test})=>test('loop',()=>{while(true){}});",
    "module.exports=({test})=>test('output',()=>{while(true)console.log('x'.repeat(10000));});",
])
def test_dom_hung_or_overoutput_process_is_bounded_and_reaped(run_dom, content):
    result, *_ = run_dom(content, timeout=2)
    assert result["status"] == "fail", result
    assert result["tests_run"] == 0
    assert "time limit" in result["detail"] or "output limit" in result["detail"]


def small_dependency_source(tmp_path, monkeypatch):
    base = tmp_path / "dependency-source"
    folder = base / "node_modules/jsdom"
    folder.mkdir(parents=True)
    (folder / "package.json").write_text('{"version":"24.1.0"}')
    (folder / "index.js").write_text("module.exports={};\n")
    rows, _, digest = dom_check._package_files(folder)
    monkeypatch.setattr(dom_check, "DEPENDENCIES", {"node_modules/jsdom": {
        "version":"24.1.0", "sha256":digest,"files":len(rows),"bytes":sum(row[3] for row in rows)}})
    return base, folder


def test_dependency_copy_authenticates_bytes_and_excludes_unrelated_parent(tmp_path, monkeypatch):
    base, folder = small_dependency_source(tmp_path, monkeypatch)
    (base / "private.txt").write_text("outside-closure")
    extra = base / "node_modules/unrelated"
    extra.mkdir(); (extra / "secret.js").write_text("outside-closure")
    staging = tmp_path / "staging"; staging.mkdir()
    destination, digest = dom_check.prepare_dependencies(staging, source=base)
    assert digest and (destination / "node_modules/jsdom/index.js").read_bytes() == (folder / "index.js").read_bytes()
    assert not (destination / "private.txt").exists()
    assert not (destination / "node_modules/unrelated").exists()
    assert not (destination / "node_modules/jsdom/index.js").stat().st_mode & 0o222


def test_standalone_dependencies_select_only_verified_bundled_closure(tmp_path, monkeypatch):
    from launchkit import distribution
    root = tmp_path / "unpacked"
    root.mkdir()
    (root / "standalone-manifest.json").write_text("{}")
    verified = []
    monkeypatch.setattr(distribution, "verify", lambda path: verified.append(path))
    assert dom_check._dependency_source(root) == root / "checker-dependencies"
    assert verified == [root]


def test_invalid_standalone_dependencies_never_fall_back_to_checkout(tmp_path, monkeypatch):
    from launchkit import distribution
    root = tmp_path / "unpacked"
    root.mkdir()
    (root / "standalone-manifest.json").symlink_to(root / "missing-manifest")
    def reject(path):
        raise distribution.DistributionError("Changed bundle")
    monkeypatch.setattr(distribution, "verify", reject)
    with pytest.raises(dom_check.DOMUnavailable, match="fresh bundle"):
        dom_check._dependency_source(root)


def test_source_checkout_dependencies_keep_the_existing_console_root(tmp_path):
    root = tmp_path / "installer"
    root.mkdir()
    assert dom_check._dependency_source(root) == tmp_path / "console"


@pytest.mark.parametrize("tamper", ["bytes", "extra", "file-link", "directory-link", "missing"])
def test_changed_missing_or_linked_dependencies_are_unavailable(tmp_path, monkeypatch, tamper):
    base, folder = small_dependency_source(tmp_path, monkeypatch)
    if tamper == "bytes":
        (folder / "index.js").write_text("changed")
    elif tamper == "extra":
        (folder / "extra.js").write_text("changed")
    elif tamper == "file-link":
        (folder / "index.js").unlink(); (folder / "index.js").symlink_to(folder / "package.json")
    elif tamper == "directory-link":
        (folder / "escape").symlink_to(base, target_is_directory=True)
    else:
        (folder / "index.js").unlink()
    staging = tmp_path / "staging"; staging.mkdir()
    with pytest.raises(dom_check.DOMUnavailable):
        dom_check.prepare_dependencies(staging, source=base)
    assert not (staging / "dom-dependencies").exists()


def test_no_runtime_or_dependency_closure_never_launches(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(dom_check,"resolve_node",lambda configured: None)
    result=dom_check.run_dom_tests(["test_x.dom.cjs"],tmp_path,tmp_path,tmp_path,tmp_path,lambda *args:calls.append(args))
    assert result["status"] == "unavailable" and not calls
    monkeypatch.setattr(dom_check,"resolve_node",lambda configured: "/usr/bin/node")
    monkeypatch.setattr(dom_check,"prepare_dependencies",lambda *args: (_ for _ in ()).throw(dom_check.DOMUnavailable("Pinned dependencies unavailable.")))
    result=dom_check.run_dom_tests(["test_x.dom.cjs"],tmp_path,tmp_path,tmp_path,tmp_path,lambda *args:calls.append(args))
    assert result["status"] == "unavailable" and not calls


@pytest.mark.parametrize("value", [
    {"tests_run": True,"test_results":[]}, {"tests_run": 0,"test_results":[]},
    {"tests_run": 2,"test_results":[{"id":"test_x.dom.cjs::one","file":"test_x.dom.cjs","status":"pass"}]},
    {"tests_run": 1,"test_results":[{"id":"test_other.dom.cjs::one","file":"test_other.dom.cjs","status":"pass"}]},
    {"tests_run": 1,"test_results":[{"id":"wrong-id","file":"test_x.dom.cjs","status":"pass"}]},
    {"tests_run": 1,"test_results":[{"id":"test_x.dom.cjs::one","file":"test_x.dom.cjs","status":"skipped"}]},
    {"tests_run": 1,"test_results":[None]},
])
def test_completion_counts_and_actual_file_ids_are_validated(value):
    raw={"status":"pass","detail":signed_completion(value)}
    result=dom_check.execution_result(raw,TEST_KEY,["test_x.dom.cjs"])
    assert result["status"] == "fail" and result["tests_run"] == 0


def test_duplicate_or_missing_completions_never_pass():
    value={"tests_run":1,"test_results":[{"id":"test_x.dom.cjs::one","file":"test_x.dom.cjs","status":"pass"}]}
    line=signed_completion(value)
    for detail in ("",line+line):
        assert dom_check.execution_result({"status":"pass","detail":detail},TEST_KEY,["test_x.dom.cjs"])["status"] == "fail"


@pytest.mark.parametrize("content", [
    # Exact reviewed attack: the previous runner exposed token/file in argv.
    '''const file=process.argv[7],token=process.argv[6];
const row={id:file+'::forged',file,status:'pass',detail:''};
console.log('MOBKIT_DOM_RESULT='+token+'='+JSON.stringify({tests_run:1,test_results:[row]}));process.exit(0);''',
    '''console.log('MOBKIT_DOM_RESULT='+'0'.repeat(64)+'='+JSON.stringify({tests_run:1,test_results:[]}));
module.exports=({test})=>test('real',()=>{});''',
])
def test_model_visible_argv_or_unsigned_stdout_cannot_forge_completion(run_dom, content):
    result, *_ = run_dom(content)
    assert result["status"] == "fail" and result["tests_run"] == 0
    assert "MOBKIT_DOM_RESULT=" not in result["detail"]


@pytest.mark.parametrize("interception", [
    '''process.stdout.write=()=>true;process.stdout._write=()=>{};
require('node:fs').writeSync=()=>{throw new Error('intercepted writer')};''',
    '''JSON.stringify=()=>'{"tests_run":1,"test_results":[{"status":"pass"}]}';
Object.prototype.toJSON=()=>({status:'pass'});Array.prototype.toJSON=()=>[];
Array.prototype.push=Array.prototype.map=Array.prototype.some=()=>[];''',
    '''const crypto=require('node:crypto');crypto.createHmac=()=>{throw new Error('intercepted crypto')};
crypto.Hmac.prototype.update=crypto.Hmac.prototype.digest=()=>{throw new Error('intercepted mac')};
Reflect.apply=()=>{throw new Error('intercepted apply')};''',
    '''Object.defineProperty(Array.prototype,'0',{configurable:true,set(value){if(value&&value.status)value.status='pass';}});''',
])
@pytest.mark.parametrize("fails", [False, True])
def test_captured_native_completion_preserves_actual_case_outcome_after_interception(run_dom, interception, fails):
    source = interception + "\nmodule.exports=({test})=>test('actual',()=>{" + (
        "throw new Error('actual case failure');" if fails else "") + "});"
    result, *_ = run_dom(source)
    assert result["status"] == ("fail" if fails else "pass"), result
    assert result["tests_run"] == 1
    assert result["test_results"][0]["status"] == ("fail" if fails else "pass")


def test_guarded_heap_aliases_native_key_prototypes_and_closed_input(run_dom):
    content = r'''module.exports=({test,assert,scratchRoot})=>test('all public heap aliases are guarded',async()=>{
 const v8=require('node:v8'),crypto=require('node:crypto'),fs=require('node:fs'),path=require('node:path');
 const aliases=[v8,require('v8'),process.getBuiltinModule('v8'),process.getBuiltinModule('node:v8'),await import('node:v8')];
 const methods=['queryObjects','getHeapSnapshot','writeHeapSnapshot','setFlagsFromString','setHeapSnapshotNearHeapLimit'];
 for(const alias of aliases){
   for(const name of methods)assert.throws(()=>alias[name](path.join(scratchRoot,'heap'),1),/Heap inspection is unavailable/);
   if(alias.default)for(const name of methods)assert.throws(()=>alias.default[name](),/Heap inspection is unavailable/);
 }
 assert.throws(()=>Object.defineProperty(v8,'queryObjects',{value:()=>[]}),TypeError);
 const key=crypto.createSecretKey(Buffer.alloc(32));
 for(let p=Object.getPrototypeOf(key);p!==Object.prototype;p=Object.getPrototypeOf(p)){
   assert.equal(Object.isFrozen(p),true);
   assert.throws(()=>Object.defineProperty(p,require('node:util').inspect.custom,{value:function(){return this.export()}}),TypeError);
 }
 assert.throws(()=>fs.readSync(0,Buffer.alloc(1),0,1,null),e=>['EBADF','ERR_ACCESS_DENIED'].includes(e.code));
 assert.throws(()=>process.binding('heap_utils'),e=>e.code==='ERR_ACCESS_DENIED');
 assert.throws(()=>new(require('node:inspector').Session)().connect(),e=>e.code==='ERR_ACCESS_DENIED');
 assert.throws(()=>new(require('node:worker_threads').Worker)('process.exit(0)',{eval:true}),e=>e.code==='ERR_ACCESS_DENIED');
});'''
    result, *_ = run_dom(content)
    assert result["status"] == "pass" and result["tests_run"] == 1, result


@pytest.mark.parametrize("mutation", ["payload", "signature", "unsigned", "malformed", "duplicate"])
def test_host_rejects_completion_count_signature_or_marker_mutation(mutation):
    value={"tests_run":1,"test_results":[{"id":"test_x.dom.cjs::one","file":"test_x.dom.cjs","status":"pass","detail":""}]}
    line=signed_completion(value)
    if mutation=="payload":line=line.replace('"tests_run":1','"tests_run":2')
    elif mutation=="signature":line=line.replace("MOBKIT_DOM_RESULT=", "MOBKIT_DOM_RESULT=f", 1)
    elif mutation=="unsigned":line="MOBKIT_DOM_RESULT="+json.dumps(value)+"\n"
    elif mutation=="malformed":line="MOBKIT_DOM_RESULT=broken\n"
    else:line+=line
    result=dom_check.execution_result({"status":"pass","detail":line},TEST_KEY,["test_x.dom.cjs"])
    assert result["status"] == "fail" and result["tests_run"] == 0
    assert "MOBKIT_DOM_RESULT=" not in result["detail"]


@pytest.mark.parametrize("case,reason", [
    ("missing", "missing completion marker"),
    ("duplicate", "duplicate completion markers"),
    ("malformed", "malformed completion marker"),
    ("unsigned", "malformed completion marker"),
    ("signature", "unauthenticated completion marker"),
    ("json", "invalid JSON or schema"),
    ("schema", "invalid JSON or schema"),
    ("missing_rows", "invalid JSON or schema"),
    ("count", "invalid counts or ownership"),
    ("owner", "invalid counts or ownership"),
    ("duplicate_ids", "invalid counts or ownership"),
    ("status", "invalid counts or ownership"),
    ("skipped", "invalid counts or ownership"),
    ("unavailable", "missing completion marker"),
])
def test_rejected_completion_reports_safe_reason_without_result_authority(case, reason):
    row = {"id": "test_x.dom.cjs::one", "file": "test_x.dom.cjs", "status": "pass",
           "detail": "private-marker-payload-must-stay-hidden"}
    value = {"tests_run": 1, "test_results": [row]}
    if case == "schema":
        value["private-field"] = "private-schema-value-must-stay-hidden"
    elif case == "missing_rows":
        del value["test_results"]
    elif case == "count":
        value = {"tests_run": 0, "test_results": []}
    elif case == "owner":
        row.update(file="other.dom.cjs", id="other.dom.cjs::one")
    elif case == "duplicate_ids":
        value.update(tests_run=2, test_results=[row, row])
    elif case == "status":
        row["status"] = "invented"
    elif case == "skipped":
        row["status"] = "skipped"
    payload = "{private-invalid-json" if case == "json" else json.dumps(value, separators=(",", ":"))
    signature = hmac.digest(TEST_KEY, payload.encode(), "sha256").hex()
    line = dom_check.DOM_RESULT_MARKER + signature + "=" + payload + "\n"
    if case in ("missing", "unavailable"):
        line = ""
    elif case == "duplicate":
        line += line
    elif case == "malformed":
        line = dom_check.DOM_RESULT_MARKER + "private-malformed-data\n"
    elif case == "unsigned":
        line = dom_check.DOM_RESULT_MARKER + payload + "\n"
    elif case == "signature":
        line = dom_check.DOM_RESULT_MARKER + "0" * 64 + "=" + payload + "\n"
    raw = {"status": "unavailable" if case == "unavailable" else "pass",
           "detail": "Runner notice.\n" + line}
    result = dom_check.execution_result(raw, TEST_KEY, ["test_x.dom.cjs"])
    assert result["status"] == ("unavailable" if case == "unavailable" else "fail")
    assert result["tests_run"] == 0 and result["test_results"] == []
    assert result["detail"] == ("No authenticated nonempty DOM test completion.\n"
                                + "Rejection: " + reason + ".\nRunner notice.")
    for hidden in (dom_check.DOM_RESULT_MARKER, signature, payload, TEST_KEY.hex(),
                   "private-marker-payload-must-stay-hidden", "private-malformed-data"):
        assert hidden not in result["detail"]


@pytest.mark.parametrize("status", ["pass", "fail"])
def test_authenticated_completion_preserves_actual_outcome_without_rejection(status):
    row = {"id": "test_x.dom.cjs::one", "file": "test_x.dom.cjs", "status": status,
           "detail": "actual assertion failure" if status == "fail" else ""}
    value = {"tests_run": 1, "test_results": [row]}
    result = dom_check.execution_result({"status": "pass", "detail": signed_completion(value)},
                                       TEST_KEY, ["test_x.dom.cjs"])
    assert result["status"] == status and result["tests_run"] == 1
    assert result["test_results"] == [row]
    assert result["detail"] == "\nHost DOM harness authenticated completion."


@pytest.mark.parametrize("probe_detail", ["unsupported controls", "DOM_NODE_VERSION_UNSUPPORTED"])
def test_permission_probe_failure_is_unavailable_without_authored_execution(tmp_path, monkeypatch, probe_detail):
    dependencies=tmp_path/'dependencies';dependencies.mkdir()
    monkeypatch.setattr(dom_check.shutil,"which",lambda name:"/usr/bin/node")
    monkeypatch.setattr(dom_check,"prepare_dependencies",lambda *args:(dependencies,"test-digest"))
    calls=[]
    def execute(command,reads,input_bytes=None):
        calls.append((command,input_bytes));return {"status":"pass","detail":probe_detail}
    result=dom_check.run_dom_tests(["test_x.dom.cjs"],tmp_path,tmp_path,tmp_path,tmp_path,execute)
    assert result["status"] == "unavailable" and len(calls)==1
    assert calls[0][1] is None and '--permission' in calls[0][0]
    if probe_detail == "DOM_NODE_VERSION_UNSUPPORTED":
        assert "25.9.0" in result["detail"]


def test_bounded_stdin_channel_keeps_python_default_and_cleanup(tmp_path, monkeypatch):
    import os
    import sys
    if not execution._sandbox_executable():pytest.skip('Trusted OS sandbox unavailable.')
    source=tmp_path/'source';source.mkdir();scratch=tmp_path/'scratch';scratch.mkdir()
    options=(execution._sandbox_executable(),execution._sandbox_profile(source,scratch,tmp_path),source,execution._check_environment(scratch))
    started=[]
    original_popen=execution.subprocess.Popen
    def observed_popen(*args,**kwargs):
        process=original_popen(*args,**kwargs)
        started.append(process)
        return process
    monkeypatch.setattr(execution.subprocess,'Popen',observed_popen)
    command=[str(Path(sys.executable).resolve()),'-I','-S','-c','import sys; print(len(sys.stdin.buffer.read()))']
    default=execution._run_sandboxed(command,*options,timeout=2)
    assert default=={"status":"pass","detail":"0"}
    supplied=execution._run_sandboxed(command,*options,timeout=2,input_bytes=b'synthetic-input')
    assert supplied=={"status":"pass","detail":"15"}
    hung=[str(Path(sys.executable).resolve()),'-I','-S','-c','import sys,os,time;sys.stdin.buffer.read();print(os.getpid(),flush=True);time.sleep(10)']
    result=execution._run_sandboxed(hung,*options,timeout=.3,input_bytes=b'synthetic-input')
    assert result['status']=='fail' and 'time limit' in result['detail']
    assert int(result['detail'].splitlines()[-1])>0
    with pytest.raises(execution.WorkspaceError):execution._run_sandboxed(command,*options,input_bytes=b'x'*65)
    assert len(started)==3
    # Linux stdout reports a PID inside bubblewrap's namespace, not a host PID.
    # Check the actual host-owned children and independently require that the
    # OS no longer has a waitable child. WNOHANG keeps a cleanup regression from
    # blocking this assertion on a still-running child.
    for process in started:
        assert process.returncode is not None and process.stdout.closed
        assert process.stdin is None or process.stdin.closed
        with pytest.raises(ChildProcessError):os.waitpid(process.pid,os.WNOHANG)


def test_missing_native_key_symbol_cannot_reach_signer_through_object_prototype(run_dom):
    # Node secret-key preparation probes a private symbol absent on a KeyObject.
    # A WebCrypto key exposes that symbol. The signing instance has no inherited
    # getter path, so this hook must never receive it when completion is signed.
    content=r'''module.exports=({test,assert})=>test('private signer has no inherited symbol getter',async()=>{
 const crypto=require('node:crypto');
 const web=await crypto.webcrypto.subtle.importKey('raw',Buffer.alloc(32),{name:'HMAC',hash:'SHA-256'},true,['sign']);
 const symbols=Object.getOwnPropertySymbols(web);
 const selector=symbols.find(symbol=>symbol.description==='kKeyObject');
 assert.ok(selector);
 Object.defineProperty(Object.prototype,selector,{configurable:true,get(){throw new Error('private key reference exposed');}});
});'''
    result,*_=run_dom(content)
    assert result['status']=='pass' and result['tests_run']==1,result


def test_native_handle_initialization_cannot_publish_a_forged_pass(run_dom):
    # The public Hmac constructor delegates through a mutable native prototype.
    # Initializing the host MAC after authors lets this hook sign invented rows.
    source=r'''const crypto=require('node:crypto'),fs=require('node:fs');
const own=crypto.createHmac('sha256',Buffer.alloc(32));
const selector=Object.getOwnPropertySymbols(own).find(s=>s.description==='kHandle');
const native=Object.getPrototypeOf(own[selector]);
const init=native.init,update=native.update,digest=native.digest;
native.init=function(...args){
 Reflect.apply(init,this,args);
 const file=process.argv[6];
 const body=JSON.stringify({tests_run:1,test_results:[{id:file+'::forged',file,status:'pass',detail:''}]});
 Reflect.apply(update,this,[body,'utf8']);
 const signature=Reflect.apply(digest,this,['hex']);
 fs.writeSync(1,'MOBKIT_DOM_RESULT='+signature+'='+body+'\n');process.exit(0);
};
module.exports=({test})=>test('actual failure',()=>{throw new Error('actual case failure');});'''
    result,*_=run_dom(source)
    assert result['status']=='fail' and result['tests_run']==1,result
    assert result['test_results'][0]['id']=='test_dom.dom.cjs::actual failure'


@pytest.mark.parametrize('method',['update','digest'])
def test_native_handle_prototype_getter_never_receives_private_signer(run_dom,method):
    source=r'''const crypto=require('node:crypto');
const own=crypto.createHmac('sha256',Buffer.alloc(32));
const selector=Object.getOwnPropertySymbols(own).find(s=>s.description==='kHandle');
const native=Object.getPrototypeOf(own[selector]);
Object.defineProperty(native,__METHOD__,{configurable:true,get(){throw new Error('private native signer exposed');}});
module.exports=({test})=>test('actual success',()=>{});'''.replace('__METHOD__',json.dumps(method))
    result,*_=run_dom(source)
    assert result['status']=='pass' and result['tests_run']==1,result


@pytest.mark.parametrize("host_input", [b"short", b"x" * 33])
def test_incomplete_or_extra_host_input_fails_before_authored_module_loading(run_dom, host_input):
    source="require('node:fs').writeFileSync(require('node:path').join(process.argv[4],'loaded'),'not allowed');module.exports=({test})=>test('false',()=>{});"
    result,_,_,scratch,_=run_dom(source,host_input_override=host_input)
    assert result['status']=='fail' and result['tests_run']==0
    assert not (scratch/'loaded').exists()
