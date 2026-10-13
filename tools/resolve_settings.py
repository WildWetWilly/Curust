#!/usr/bin/env python3
"""Resolve Cura setting values the way the Cura 5.12 frontend does, for testing.

Mimics Uranium's DefinitionContainer / ContainerStack / Cura's GlobalStack and ExtruderStack
property evaluation (including `resolve`, `limit_to_extruder` and the Cura formula
functions), then writes a CuraEngine "resolved settings" JSON (the `-r` command line
format) containing what Cura's StartSliceJob would send to the engine.

Usage:
  resolve_settings.py --resources <cura/resources> --machine creality_ender3 \
      [--set key=value ...] [--set-extruder N:key=value ...] -o resolved.json

`--set` values are Python literals or formulas prefixed with '=' (like Cura profiles).
"""
import argparse
import ast
import json
import math
import os
import sys

FUNCTION_PROPS = {
    "value", "enabled", "minimum_value", "maximum_value", "minimum_value_warning",
    "maximum_value_warning", "resolve", "limit_to_extruder", "allow_empty", "is_uuid",
    "warning_value", "error_value",
}
PROPERTY_DEFAULTS = {"enabled": True, "limit_to_extruder": "-1", "settable_per_extruder": True, "settable_per_meshgroup": True,
                     "settable_per_mesh": True, "settable_globally": True, "default_value": 0}


class Function:
    """Equivalent of UM.Settings.SettingFunction."""

    def __init__(self, code):
        self.code = str(code)
        self.compiled = compile(self.code, "<setting>", "eval")
        tree = ast.parse(self.code, mode="eval")
        self.names = sorted({n.id for n in ast.walk(tree) if isinstance(n, ast.Name)})

    def __call__(self, stack, ctx):
        provider = ctx.root() if ctx is not None else stack
        local_vars = {}
        for name in self.names:
            if name in OPERATOR_NAMES or name in BUILTIN_NAMES:
                continue
            if not provider.has_key(name):
                continue
            v = provider.get_property(name, "value", ctx)
            if v is None:
                continue
            local_vars[name] = v
        g = {"math": math, "__builtins__": __builtins__}
        g.update(provider.resolver.operators())
        try:
            return eval(self.compiled, g, local_vars)
        except Exception as e:  # Uranium returns 0 on exceptions
            print(f"warning: exception evaluating {self.code!r}: {e}", file=sys.stderr)
            return 0

    def __repr__(self):
        return f"={self.code}"


OPERATOR_NAMES = {"extruderValue", "extruderValues", "anyExtruderWithMaterial", "anyExtruderNrWithOrDefault",
                  "resolveOrValue", "defaultExtruderPosition", "valueFromContainer", "extruderValueFromContainer"}
BUILTIN_NAMES = set(dir(__builtins__)) | {"math"}


class Context:
    def __init__(self, root):
        self.stack = [root]
        self.context = {}

    def root(self):
        return self.stack[0]

    def push(self, s):
        self.stack.append(s)

    def pop(self):
        self.stack.pop()


def find_def(def_id, search_dirs):
    for d in search_dirs:
        p = os.path.join(d, def_id + ".def.json")
        if os.path.exists(p):
            return p
    raise FileNotFoundError(def_id)


class DefinitionContainer:
    def __init__(self, def_id, search_dirs):
        chain = []
        cur = def_id
        while cur:
            with open(find_def(cur, search_dirs)) as f:
                doc = json.load(f)
            chain.append(doc)
            cur = doc.get("inherits")
        chain.reverse()
        self.id = def_id
        self.settings = {}  # key -> dict of properties
        self.order = []
        self.metadata = {}
        for doc in chain:
            self.metadata.update(doc.get("metadata", {}))
            if "settings" in doc:
                self._add_tree(doc["settings"], None)
            for key, props in doc.get("overrides", {}).items():
                if key not in self.settings:
                    # Uranium ignores overrides of unknown settings (with a warning)
                    continue
                self.settings[key].update(props)
        for key, props in self.settings.items():
            for p in FUNCTION_PROPS:
                if p in props and not isinstance(props[p], Function):
                    props[p] = Function(props[p] if not isinstance(props[p], bool) else str(props[p]))

    def _add_tree(self, tree, parent):
        for key, props in tree.items():
            entry = {k: v for k, v in props.items() if k != "children"}
            entry["_parent"] = parent
            self.settings[key] = entry
            self.order.append(key)
            if "children" in props:
                self._add_tree(props["children"], key)

    def has_key(self, key):
        return key in self.settings

    def get(self, key, prop):
        s = self.settings.get(key)
        if s is None:
            return None
        if prop in s:
            return s[prop]
        if prop == "value":
            return s.get("default_value")
        return PROPERTY_DEFAULTS.get(prop)


class InstanceContainer:
    def __init__(self, values=None):
        self.values = {}
        for k, v in (values or {}).items():
            self.set(k, v)

    def set(self, key, value):
        if isinstance(value, str) and value.startswith("="):
            value = Function(value[1:])
        self.values[key] = value

    def has_key(self, key):
        return key in self.values

    def get(self, key, prop):
        if prop == "value":
            return self.values.get(key)
        return None


class Stack:
    def __init__(self, resolver, containers, next_stack=None, position=None):
        self.resolver = resolver
        self.containers = containers  # top first, definition last
        self.next = next_stack
        self.position = position  # None for global
        self.resolving = set()

    @property
    def definition(self):
        return self.containers[-1]

    def has_key(self, key):
        if any(c.has_key(key) for c in self.containers):
            return True
        return self.next.has_key(key) if self.next else False

    def all_keys(self):
        keys = list(self.definition.order)
        if self.next:
            seen = set(keys)
            keys += [k for k in self.next.all_keys() if k not in seen]
        return keys

    def get_raw(self, key, prop):
        for c in self.containers:
            v = c.get(key, prop)
            if v is not None:
                return v
        if self.next is not None:
            return self.next.get_raw(key, prop)
        return None

    def _base_get_property(self, key, prop, ctx):
        """Uranium ContainerStack.getProperty."""
        v = self.get_raw(key, prop)
        if isinstance(v, Function):
            if ctx is not None:
                ctx.push(self)
            v = v(self, ctx)
            if ctx is not None:
                ctx.pop()
        return v

    def has_user_value(self, key):
        return self.containers[0].has_key(key)

    def get_property(self, key, prop, ctx=None):
        if self.position is None:
            return self._global_get_property(key, prop, ctx)
        return self._extruder_get_property(key, prop, ctx)

    # cura.Settings.GlobalStack.getProperty
    def _global_get_property(self, key, prop, ctx):
        if not self.definition.has_key(key):
            return None
        if ctx is not None:
            ctx.push(self)
        if prop == "value" and self.definition.get(key, "resolve") is not None and key not in self.resolving and not self.has_user_value(key):
            self.resolving.add(key)
            resolve = self._base_get_property(key, "resolve", ctx)
            self.resolving.discard(key)
            if resolve is not None:
                if ctx is not None:
                    ctx.pop()
                return resolve
        lte = self._base_get_property(key, "limit_to_extruder", ctx)
        if lte is not None:
            lte = str(lte)
        extruders = self.resolver.extruder_stacks
        if lte is not None and lte != "-1" and lte in extruders:
            if self._base_get_property(key, "settable_per_extruder", ctx):
                result = extruders[lte].get_property(key, prop, ctx)
                if result is not None:
                    if ctx is not None:
                        ctx.pop()
                    return result
        result = self._base_get_property(key, prop, ctx)
        if ctx is not None:
            ctx.pop()
        return result

    # cura.Settings.ExtruderStack.getProperty
    def _extruder_get_property(self, key, prop, ctx):
        if ctx is not None:
            ctx.push(self)
        if not self._base_get_property(key, "settable_per_extruder", ctx):
            result = self.next.get_property(key, prop, ctx)
            if ctx is not None:
                ctx.pop()
            return result
        if ctx is None:
            ctx = Context(self)
        if "extruder_position" not in ctx.context:
            ctx.context["extruder_position"] = self._base_get_property(key, "limit_to_extruder", ctx)
        lte = ctx.context["extruder_position"]
        if lte is not None:
            lte = str(lte)
        if lte is not None and lte != "-1" and str(self.position) != lte:
            ext = self.resolver.extruder_list()
            try:
                result = ext[int(float(lte))].get_property(key, prop, ctx)
                if result is not None:
                    ctx.pop()
                    return result
            except IndexError:
                pass
        result = self._base_get_property(key, prop, ctx)
        ctx.pop()
        return result


class Resolver:
    def __init__(self, resources, machine, user_global, user_extruders):
        self.search_dirs = [os.path.join(resources, "definitions"), os.path.join(resources, "extruders")]
        self.machine_def = DefinitionContainer(machine, self.search_dirs)
        self.global_stack = Stack(self, [InstanceContainer(user_global), self.machine_def])
        trains = self.machine_def.metadata.get("machine_extruder_trains", {"0": "fdmextruder"})
        self.extruder_stacks = {}
        for pos, ext_id in sorted(trains.items(), key=lambda kv: int(kv[0])):
            ext_def = DefinitionContainer(ext_id, self.search_dirs)
            user = dict(user_extruders.get(int(pos), {}))
            self.extruder_stacks[str(pos)] = Stack(self, [InstanceContainer(user), ext_def], self.global_stack, position=int(pos))

    def extruder_list(self):
        count = self.global_stack.get_property("machine_extruder_count", "value")
        return [self.extruder_stacks[str(i)] for i in range(int(count)) if str(i) in self.extruder_stacks]

    def active_extruders(self):
        return [e for e in self.extruder_list() if e.get_property("extruder_enabled", "value") is not False]

    def default_extruder_position(self):
        act = self.active_extruders()
        return str(act[0].position) if act else "0"

    def operators(self):
        def extruder_value(extruder_position, property_key, context=None):
            if extruder_position == -1:
                extruder_position = int(self.default_extruder_position())
            lst = self.extruder_list()
            try:
                stack = lst[int(extruder_position)]
            except IndexError:
                if extruder_position != 0:
                    return extruder_value(0, property_key, context)
                return None
            value = stack.get_raw(property_key, "value")
            if isinstance(value, Function):
                value = value(stack, context)
            if isinstance(value, str):
                value = value.lower()
            return value

        def extruder_values(property_key, context=None):
            result = []
            for stack in self.active_extruders():
                value = stack.get_raw(property_key, "value")
                if value is None:
                    continue
                if isinstance(value, Function):
                    value = value(stack, context)
                result.append(value)
            if not result:
                result.append(self.global_stack.get_property(property_key, "value", context))
            return result

        def any_extruder_with_material(filter_key, context=None):
            return self.default_extruder_position()

        def any_extruder_nr_with_or_default(filter_key, context=None):
            for stack in self.active_extruders():
                value = stack.get_raw(filter_key, "value")
                if value is None or not value:
                    continue
                return str(stack.position)

        def resolve_or_value(property_key, context=None):
            return self.global_stack.get_property(property_key, "value", context)

        return {
            "extruderValue": extruder_value,
            "extruderValues": extruder_values,
            "anyExtruderWithMaterial": any_extruder_with_material,
            "anyExtruderNrWithOrDefault": any_extruder_nr_with_or_default,
            "resolveOrValue": resolve_or_value,
            "defaultExtruderPosition": self.default_extruder_position,
        }

    # --- StartSliceJob equivalents -------------------------------------------------------
    def replacement_tokens(self, stack):
        result = {}
        for key in stack.all_keys():
            result[key] = stack.get_property(key, "value")
        for k in ("material_id", "material_type", "material_name", "material_brand", "quality_name"):
            result[k] = ""
        result["quality_changes_name"] = None
        result["print_bed_temperature"] = result.get("material_bed_temperature")
        result["print_temperature"] = result.get("material_print_temperature")
        result["travel_speed"] = result.get("speed_travel")
        result["time"] = "00:00:00"
        result["date"] = "01-01-2026"
        result["day"] = "Thu"
        result["initial_extruder_nr"] = int(self.default_extruder_position())
        return result

    def expand_tokens(self, text, tokens_by_extruder, default_nr):
        import re

        def repl(m):
            key = m.group(1)
            nr = m.group(2)
            tokens = tokens_by_extruder.get(str(int(nr)) if nr is not None else str(default_nr), tokens_by_extruder["-1"])
            if key in tokens:
                return str(tokens[key])
            return m.group(0)

        return re.sub(r"\{(\w+)(?:,\s*(-?\d+))?\}", repl, text)

    def build(self):
        tokens = {"-1": self.replacement_tokens(self.global_stack)}
        for stack in self.active_extruders():
            nr = stack.get_property("extruder_nr", "value")
            tokens[str(nr)] = self.replacement_tokens(stack)
        out = {}
        g = dict(tokens["-1"])
        init_nr = int(self.default_extruder_position())
        g["machine_start_gcode"] = self.expand_tokens(g["machine_start_gcode"], tokens, init_nr)
        g["machine_end_gcode"] = self.expand_tokens(g["machine_end_gcode"], tokens, init_nr)
        g["nozzle_offsetting_for_disallowed_areas"] = self.machine_def.metadata.get("nozzle_offsetting_for_disallowed_areas", True)
        out["global"] = {k: str(v) for k, v in g.items()}
        gdef = self.global_stack.definition
        for stack in self.active_extruders():
            nr = stack.get_property("extruder_nr", "value")
            s = dict(tokens[str(nr)])
            s["material_guid"] = ""
            for k in ("machine_extruder_prestart_code", "machine_extruder_start_code", "machine_extruder_end_code"):
                s[k] = self.expand_tokens(s[k], tokens, nr)
            ext = {}
            for k, v in s.items():
                if not gdef.get(k, "settable_per_extruder") and not stack.definition.get(k, "settable_per_extruder"):
                    continue
                ext[k] = str(v)
            out[f"extruder.{stack.position}"] = ext
        lte = {}
        for key in self.global_stack.all_keys():
            p = self.global_stack.get_property(key, "limit_to_extruder")
            pos = int(round(float(p)))
            if pos >= 0:
                lte[key] = f"extruder.{pos}"
        out["limit_to_extruder"] = lte
        return out


def parse_value(v):
    if v.startswith("="):
        return v
    try:
        return ast.literal_eval(v)
    except Exception:
        return v


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--resources", required=True)
    ap.add_argument("--machine", default="fdmprinter")
    ap.add_argument("--set", action="append", default=[])
    ap.add_argument("--set-extruder", action="append", default=[])
    ap.add_argument("-o", "--output", required=True)
    args = ap.parse_args()
    user_global = {}
    for s in args.set:
        k, v = s.split("=", 1)
        user_global[k] = parse_value(v)
    user_ext = {}
    for s in args.set_extruder:
        nr, kv = s.split(":", 1)
        k, v = kv.split("=", 1)
        user_ext.setdefault(int(nr), {})[k] = parse_value(v)
    r = Resolver(args.resources, args.machine, user_global, user_ext)
    out = r.build()
    with open(args.output, "w") as f:
        json.dump(out, f, indent=1, sort_keys=True)


if __name__ == "__main__":
    main()
