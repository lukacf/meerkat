"""Every name used in an annotation must be bound where the annotation stands.

The package is typed (``py.typed``) and its modules use postponed
annotations, so an annotation that names a class imported only inside a
function body raises nothing at run time. A type checker then reads the
annotation as unknown, and a caller loses checking on the value: with
``MetaBuilder.build`` that was the whole runtime object.

Checked: the signatures of functions and methods at any depth, and annotated
assignments (module variables, class attributes, locals). Not checked: the
string arguments of ``typing.cast`` and ``TypeVar`` bounds.
"""
from __future__ import annotations

import ast
import builtins
from pathlib import Path

import meerkat_meta

PACKAGE_DIR = Path(meerkat_meta.__file__).parent

_FUNCTIONS = (ast.FunctionDef, ast.AsyncFunctionDef)


def _child_bodies(node: ast.stmt) -> list[list[ast.stmt]]:
    """The statement lists nested in a compound statement, same scope."""
    bodies: list[list[ast.stmt]] = []
    for name in ("body", "orelse", "finalbody"):
        body = getattr(node, name, None)
        if isinstance(body, list):
            bodies.append(body)
    for handler in getattr(node, "handlers", []):
        bodies.append(handler.body)
    for case in getattr(node, "cases", []):
        bodies.append(case.body)
    return bodies


def _bound_names(body: list[ast.stmt]) -> set[str]:
    """Names a module, class or function body binds in its own scope."""
    names: set[str] = set()
    for node in body:
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            for alias in node.names:
                names.add((alias.asname or alias.name).split(".")[0])
        elif isinstance(node, (ast.ClassDef, *_FUNCTIONS)):
            names.add(node.name)
        elif isinstance(node, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                for leaf in ast.walk(target):
                    if isinstance(leaf, ast.Name):
                        names.add(leaf.id)
        else:
            for child in _child_bodies(node):
                names |= _bound_names(child)
    return names


def _is_typing_name(node: ast.expr, name: str) -> bool:
    return (isinstance(node, ast.Name) and node.id == name) or (
        isinstance(node, ast.Attribute) and node.attr == name
    )


def _annotation_names(annotation: ast.expr) -> set[str]:
    """Names an annotation refers to; quoted annotations are parsed too."""
    if isinstance(annotation, ast.Name):
        return {annotation.id}
    if isinstance(annotation, ast.Constant):
        if not isinstance(annotation.value, str):
            return set()
        try:
            quoted = ast.parse(annotation.value, mode="eval")
        except SyntaxError:
            return set()
        return _annotation_names(quoted.body)
    if isinstance(annotation, ast.Subscript):
        names = _annotation_names(annotation.value)
        if _is_typing_name(annotation.value, "Literal"):
            return names  # the arguments of Literal are values, not types
        arguments = annotation.slice
        if _is_typing_name(annotation.value, "Annotated"):
            # Only the first argument of Annotated is a type.
            if isinstance(arguments, ast.Tuple) and arguments.elts:
                arguments = arguments.elts[0]
        return names | _annotation_names(arguments)
    names = set()
    for child in ast.iter_child_nodes(annotation):
        if isinstance(child, ast.expr):
            names |= _annotation_names(child)
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


def _type_parameters(node: ast.AST) -> set[str]:
    return {parameter.name for parameter in getattr(node, "type_params", [])}


def _unbound_in(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    problems: list[str] = []

    def report(node: ast.stmt, label: str, annotation: ast.expr, scope: set[str]) -> None:
        for name in sorted(_annotation_names(annotation) - scope):
            problems.append(f"{path.name}:{node.lineno} {label}: {name}")

    def visit(body: list[ast.stmt], scope: set[str]) -> None:
        for node in body:
            if isinstance(node, ast.ClassDef):
                inner = scope | _type_parameters(node) | _bound_names(node.body)
                visit(node.body, inner)
            elif isinstance(node, _FUNCTIONS):
                own = scope | _type_parameters(node)
                for annotation in _signature_annotations(node):
                    report(node, node.name, annotation, own)
                # A nested function sees what the enclosing function binds.
                visit(node.body, own | _bound_names(node.body))
            elif isinstance(node, ast.AnnAssign):
                target = ast.unparse(node.target)
                report(node, target, node.annotation, scope)
            else:
                for child in _child_bodies(node):
                    visit(child, scope)

    visit(tree.body, _bound_names(tree.body) | set(dir(builtins)))
    return problems


def _sample(tmp_path: Path, source: str) -> Path:
    module = tmp_path / "sample.py"
    module.write_text("from __future__ import annotations\n" + source, encoding="utf-8")
    return module


def test_annotations_name_only_bound_names() -> None:
    paths = sorted(PACKAGE_DIR.rglob("*.py"))
    # The package has modules in a subpackage; make sure they are read.
    assert any(path.parent != PACKAGE_DIR for path in paths)
    problems: list[str] = []
    for path in paths:
        problems.extend(_unbound_in(path))
    assert problems == []


def test_the_check_reports_a_name_imported_only_in_the_body(tmp_path: Path) -> None:
    module = _sample(
        tmp_path,
        "def build() -> Thing:\n"
        "    from elsewhere import Thing\n"
        "    return Thing()\n",
    )
    assert _unbound_in(module) == ["sample.py:2 build: Thing"]


def test_the_check_reads_nested_functions_and_attribute_annotations(tmp_path: Path) -> None:
    module = _sample(
        tmp_path,
        "class Holder:\n"
        "    item: Missing\n"
        "    def outer(self) -> None:\n"
        "        from elsewhere import Local\n"
        "        def inner(value: Local) -> Other:\n"
        "            ...\n",
    )
    assert _unbound_in(module) == [
        "sample.py:3 item: Missing",
        "sample.py:6 inner: Other",
    ]


def test_the_check_does_not_read_literal_values_as_names(tmp_path: Path) -> None:
    module = _sample(
        tmp_path,
        "import typing\n"
        "from typing import Annotated, Literal\n"
        "def pick(mode: Literal[\"fast\", \"slow\"], other: typing.Literal[\"x\"]) -> \"Literal[\\\"done\\\"]\":\n"
        "    ...\n"
        "def note(value: Annotated[int, \"units\"]) -> None:\n"
        "    ...\n",
    )
    assert _unbound_in(module) == []
