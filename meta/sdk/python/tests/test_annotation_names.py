"""Every name used in a signature annotation must be bound in its module.

The package is typed (``py.typed``) and its modules use postponed
annotations, so an annotation that names a class imported only inside the
function body raises nothing at run time. A type checker then reads the
annotation as unknown, and a caller loses checking on the value: with
``MetaBuilder.build`` that was the whole runtime object.
"""
from __future__ import annotations

import ast
import builtins
from pathlib import Path

import meerkat_meta

PACKAGE_DIR = Path(meerkat_meta.__file__).parent


def _bound_names(body: list[ast.stmt]) -> set[str]:
    """Names a module or class body binds, including under ``if``/``try``."""
    names: set[str] = set()
    for node in body:
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            for alias in node.names:
                names.add((alias.asname or alias.name).split(".")[0])
        elif isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            names.add(node.name)
        elif isinstance(node, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                for leaf in ast.walk(target):
                    if isinstance(leaf, ast.Name):
                        names.add(leaf.id)
        elif isinstance(node, ast.If):
            names |= _bound_names(node.body) | _bound_names(node.orelse)
        elif isinstance(node, ast.Try):
            names |= _bound_names(node.body) | _bound_names(node.orelse)
            names |= _bound_names(node.finalbody)
            for handler in node.handlers:
                names |= _bound_names(handler.body)
    return names


def _annotation_names(annotation: ast.expr) -> set[str]:
    """Names an annotation refers to; quoted annotations are parsed too."""
    names: set[str] = set()
    for leaf in ast.walk(annotation):
        if isinstance(leaf, ast.Name):
            names.add(leaf.id)
        elif isinstance(leaf, ast.Constant) and isinstance(leaf.value, str):
            try:
                quoted = ast.parse(leaf.value, mode="eval")
            except SyntaxError:
                continue  # a string literal inside Literal[...], not a type
            for inner in ast.walk(quoted):
                if isinstance(inner, ast.Name):
                    names.add(inner.id)
    return names


def _signature_annotations(function: ast.FunctionDef | ast.AsyncFunctionDef):
    arguments = function.args
    for argument in (
        *arguments.posonlyargs,
        *arguments.args,
        *arguments.kwonlyargs,
        arguments.vararg,
        arguments.kwarg,
    ):
        if argument is not None and argument.annotation is not None:
            yield argument.annotation
    if function.returns is not None:
        yield function.returns


def _unbound_in(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    module_names = _bound_names(tree.body) | set(dir(builtins))
    problems: list[str] = []

    def visit(body: list[ast.stmt], scope: set[str]) -> None:
        for node in body:
            if isinstance(node, ast.ClassDef):
                visit(node.body, scope | _bound_names(node.body))
            elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                for annotation in _signature_annotations(node):
                    for name in sorted(_annotation_names(annotation) - scope):
                        problems.append(f"{path.name}:{node.lineno} {node.name}: {name}")
            elif isinstance(node, ast.If):
                visit(node.body, scope)
                visit(node.orelse, scope)

    visit(tree.body, module_names)
    return problems


def test_signature_annotations_name_only_bound_names() -> None:
    problems: list[str] = []
    for path in sorted(PACKAGE_DIR.glob("*.py")):
        problems.extend(_unbound_in(path))
    assert problems == []


def test_the_check_reports_a_name_imported_only_in_the_body(tmp_path: Path) -> None:
    module = tmp_path / "sample.py"
    module.write_text(
        "from __future__ import annotations\n"
        "def build() -> Thing:\n"
        "    from elsewhere import Thing\n"
        "    return Thing()\n",
        encoding="utf-8",
    )
    assert _unbound_in(module) == ["sample.py:2 build: Thing"]
