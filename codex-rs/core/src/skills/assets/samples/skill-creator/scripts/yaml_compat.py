#!/usr/bin/env python3
"""YAML compatibility layer for skill-creator scripts.

Prefer PyYAML when available. Fall back to a strict, built-in parser that
supports the SKILL.md frontmatter subset used by this skill.
"""

from __future__ import annotations

import ast
import re
from typing import Any

try:
    import yaml as _pyyaml
except ModuleNotFoundError:
    _pyyaml = None


if _pyyaml is not None:
    BACKEND = "pyyaml"
    YAMLError = _pyyaml.YAMLError

    def safe_load(text: str) -> Any:
        return _pyyaml.safe_load(text)

else:
    BACKEND = "shim"

    class YAMLError(Exception):
        pass

    _KEY_PATTERN = re.compile(r"^([A-Za-z0-9_-]+):(.*)$")

    def _parse_scalar(raw_value: str) -> Any:
        value = raw_value.strip()
        if value == "":
            return ""
        if value in {"null", "Null", "NULL", "~"}:
            return None
        if value in {"true", "True", "TRUE"}:
            return True
        if value in {"false", "False", "FALSE"}:
            return False
        if re.fullmatch(r"-?\d+", value):
            return int(value)
        if value.startswith('"') and value.endswith('"'):
            try:
                return ast.literal_eval(value)
            except Exception as exc:
                raise YAMLError(f"invalid double-quoted string: {value}") from exc
        if value.startswith("'") and value.endswith("'"):
            return value[1:-1].replace("''", "'")
        if value.startswith("[") and value.endswith("]"):
            inner = value[1:-1].strip()
            if not inner:
                return []
            parts = [part.strip() for part in inner.split(",")]
            return [_parse_scalar(part) for part in parts]
        return value

    def _parse_yaml_mapping_shim(text: str) -> dict[str, Any]:
        root: dict[str, Any] = {}
        stack: list[tuple[int, dict[str, Any]]] = [(-1, root)]

        for line_no, raw_line in enumerate(text.splitlines(), start=1):
            if raw_line.strip() == "" or raw_line.lstrip().startswith("#"):
                continue

            indent = len(raw_line) - len(raw_line.lstrip(" "))
            if raw_line[:indent].count("\t") > 0:
                raise YAMLError(f"line {line_no}: tabs are not supported")
            if indent % 2 != 0:
                raise YAMLError(f"line {line_no}: indentation must use 2-space levels")

            stripped = raw_line.strip()
            match = _KEY_PATTERN.match(stripped)
            if not match:
                raise YAMLError(
                    f"line {line_no}: unsupported YAML syntax: {raw_line.strip()}"
                )

            key = match.group(1)
            value_text = match.group(2).strip()

            while len(stack) > 1 and indent <= stack[-1][0]:
                stack.pop()

            parent = stack[-1][1]
            if value_text == "":
                child: dict[str, Any] = {}
                parent[key] = child
                stack.append((indent, child))
            else:
                parent[key] = _parse_scalar(value_text)

        return root

    def safe_load(text: str) -> Any:
        return _parse_yaml_mapping_shim(text)

