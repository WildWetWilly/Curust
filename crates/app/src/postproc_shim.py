# Curust's stand-ins for the Uranium / Cura / PyQt APIs that Cura's post-processing scripts use.
# The setting-container classes follow Uranium's semantics (SettingInstance type conversion,
# SettingFunction evaluation, ContainerStack lookup order); application state comes from the
# host module `_curust_host`.
import _curust_host as _h
import json
import re


# ---------------------------------------------------------------- UM.Signal / UM.i18n / UM.Logger

class Signal:
    Direct = 1
    Auto = 2
    Queued = 3

    def __init__(self, *args, **kwargs):
        self._slots = []

    def connect(self, slot):
        self._slots.append(slot)

    def disconnect(self, slot):
        for k, s in enumerate(self._slots):
            if s == slot:
                del self._slots[k]
                return

    def disconnectAll(self):
        self._slots = []

    def emit(self, *args, **kwargs):
        for s in list(self._slots):
            s(*args, **kwargs)


def signalemitter(cls):
    return cls


class i18nCatalog:
    def __init__(self, name=None, language="default"):
        self._name = name

    def hasTranslationLoaded(self):
        return False

    def _fmt(self, text, args):
        if args:
            return text.format(*args)
        return text

    def i18n(self, text, *args):
        return self._fmt(text, args)

    def i18nc(self, context, text, *args):
        return self._fmt(text, args)

    def i18np(self, single, multiple, counter, *args):
        return self._fmt(single if counter == 1 else multiple, (counter,) + tuple(args))

    def i18ncp(self, context, single, multiple, counter, *args):
        return self._fmt(single if counter == 1 else multiple, (counter,) + tuple(args))


class Logger:
    @classmethod
    def log(cls, log_type, message, *args, **kwargs):
        if args:
            message = message % args
        _h.log(log_type, str(message))

    @classmethod
    def logException(cls, log_type, message, *args):
        if args:
            message = message % args
        _h.log_exception(log_type, str(message))

    @classmethod
    def debug(cls, message, *args):
        cls.log("d", message, *args)

    @classmethod
    def info(cls, message, *args):
        cls.log("i", message, *args)

    @classmethod
    def warning(cls, message, *args):
        cls.log("w", message, *args)

    @classmethod
    def warn(cls, message, *args):
        cls.log("w", message, *args)

    @classmethod
    def error(cls, message, *args):
        cls.log("e", message, *args)

    @classmethod
    def critical(cls, message, *args):
        cls.log("c", message, *args)


class Message:
    class MessageType:
        POSITIVE = 1
        NEUTRAL = 0
        WARNING = 2
        ERROR = 3

    def __init__(self, text="", lifetime=30, dismissable=True, progress=None, title=None, parent=None, use_inactivity_timer=True, image_source="", image_caption="", option_text="", option_state=True, message_type=0, **kwargs):
        self._text = text
        self._title = title
        self._lifetime = lifetime
        self._message_type = message_type
        self.actionTriggered = Signal()
        self.optionToggled = Signal()

    def show(self):
        _h.message("" if self._title is None else str(self._title), str(self._text), int(self._message_type) if isinstance(self._message_type, int) else 0)

    def hide(self, send_signal=True):
        pass

    def addAction(self, *args, **kwargs):
        pass

    def getText(self):
        return self._text

    def setText(self, text):
        self._text = text

    def getTitle(self):
        return self._title

    def setTitle(self, title):
        self._title = title

    def setProgress(self, progress):
        pass

    def setLifetimeTimer(self, *args):
        pass


# ---------------------------------------------------------------- UM.Math.Vector

class Vector:
    def __init__(self, x=0.0, y=0.0, z=0.0, data=None):
        if data is not None:
            x, y, z = data[0], data[1], data[2]
        self._x = float(x)
        self._y = float(y)
        self._z = float(z)

    @property
    def x(self):
        return self._x

    @property
    def y(self):
        return self._y

    @property
    def z(self):
        return self._z

    def set(self, x=None, y=None, z=None):
        return Vector(self._x if x is None else x, self._y if y is None else y, self._z if z is None else z)

    def length(self):
        return (self._x * self._x + self._y * self._y + self._z * self._z) ** 0.5

    def normalized(self):
        n = self.length()
        if n == 0:
            return Vector(self._x, self._y, self._z)
        return Vector(self._x / n, self._y / n, self._z / n)

    def dot(self, o):
        return self._x * o.x + self._y * o.y + self._z * o.z

    def cross(self, o):
        return Vector(self._y * o.z - self._z * o.y, self._z * o.x - self._x * o.z, self._x * o.y - self._y * o.x)

    def angleToVector(self, o):
        import math
        d = self.normalized().dot(o.normalized())
        return math.acos(max(-1.0, min(1.0, d)))

    def __add__(self, o):
        if isinstance(o, Vector):
            return Vector(self._x + o.x, self._y + o.y, self._z + o.z)
        return Vector(self._x + o, self._y + o, self._z + o)

    def __sub__(self, o):
        if isinstance(o, Vector):
            return Vector(self._x - o.x, self._y - o.y, self._z - o.z)
        return Vector(self._x - o, self._y - o, self._z - o)

    def __mul__(self, o):
        if isinstance(o, Vector):
            return Vector(self._x * o.x, self._y * o.y, self._z * o.z)
        return Vector(self._x * o, self._y * o, self._z * o)

    def __rmul__(self, o):
        return self.__mul__(o)

    def __truediv__(self, o):
        if isinstance(o, Vector):
            return Vector(self._x / o.x, self._y / o.y, self._z / o.z)
        return Vector(self._x / o, self._y / o, self._z / o)

    def __neg__(self):
        return Vector(-self._x, -self._y, -self._z)

    def __eq__(self, o):
        return isinstance(o, Vector) and self._x == o.x and self._y == o.y and self._z == o.z

    def __repr__(self):
        return "Vector({0:.3f}, {1:.3f}, {2:.3f})".format(self._x, self._y, self._z)

    def __str__(self):
        return self.__repr__()


Vector.Null = Vector(0, 0, 0)
Vector.Unit_X = Vector(1, 0, 0)
Vector.Unit_Y = Vector(0, 1, 0)
Vector.Unit_Z = Vector(0, 0, 1)


# ---------------------------------------------------------------- UM.Qt.Duration

class DurationFormat:
    class Format:
        Seconds = 0
        Short = 1
        Long = 2
        ISO8601 = 3


class Duration:
    def __init__(self, duration=None, parent=None):
        self._days = -1
        self._hours = -1
        self._minutes = -1
        self._seconds = -1
        if duration is not None:
            self.setDuration(duration)

    def setDuration(self, duration):
        import math
        if duration < 0:
            self._days = self._hours = self._minutes = self._seconds = -1
            return
        duration = round(duration)
        if int(duration) >= 2 ** 31:
            duration = 0
        self._days = math.floor(duration / (3600 * 24))
        duration -= self._days * 3600 * 24
        self._hours = math.floor(duration / 3600)
        duration -= self._hours * 3600
        self._minutes = math.floor(duration / 60)
        duration -= self._minutes * 60
        self._seconds = duration

    @property
    def days(self):
        return self._days

    @property
    def hours(self):
        return self._hours

    @property
    def minutes(self):
        return self._minutes

    @property
    def seconds(self):
        return self._seconds

    @property
    def valid(self):
        return self._days != -1 and self._hours != -1 and self._minutes != -1 and self._seconds != -1

    @property
    def isTotalDurationZero(self):
        return self._days == 0 and self._hours == 0 and self._minutes == 0 and self._seconds == 0

    def getDisplayString(self, display_format=DurationFormat.Format.Short):
        if display_format == DurationFormat.Format.Seconds:
            return str(((self._days * 24 + self._hours) * 60 + self._minutes) * 60 + self._seconds)
        elif display_format == DurationFormat.Format.Short:
            return "{0:0>2}h {1:0>2}min".format(self._days * 24 + self._hours, self._minutes)
        elif display_format == DurationFormat.Format.Long:
            if self._days > 0:
                return "{0} days {1} hours {2} minutes".format(self._days, self._hours, self._minutes)
            elif self._hours > 0:
                return "{0} hours {1} minutes".format(self._hours, self._minutes)
            return "{0} minutes".format(self._minutes)
        elif display_format == DurationFormat.Format.ISO8601:
            return "%02d:%02d:%02d" % (self._days * 24 + self._hours, self._minutes, self._seconds)
        return ""

    def __int__(self):
        return ((self._days * 24 + self._hours) * 60 + self._minutes) * 60 + self._seconds


# ---------------------------------------------------------------- setting containers (Uranium)

def _literal_eval(text):
    return _h.literal_eval(text.lstrip(" \t"))


def toFloatConversion(value):
    value = value.replace(",", ".")

    def stripLeading0(matchobj):
        return matchobj.group(0).lstrip("0")

    regex_pattern = r"(?<!\.|\w|\d)0+(\d+)"
    value = re.sub(regex_pattern, stripLeading0, value)
    try:
        return _literal_eval(value)
    except:
        return 0


def toIntConversion(value):
    try:
        return _literal_eval(value)
    except SyntaxError:
        return 0


_FROM_STRING = {
    "int": toIntConversion,
    "bool": _literal_eval,
    "category": None,
    "str": None,
    "enum": None,
    "float": toFloatConversion,
    "polygon": _literal_eval,
    "polygons": _literal_eval,
    "vec3": None,
}


def settingValueFromString(type_name, string_value):
    if type_name not in _FROM_STRING:
        raise ValueError("Unknown setting type {0}".format(type_name))
    f = _FROM_STRING[type_name]
    if f:
        return f(string_value)
    return string_value


_FUNCTION_PROPERTIES = ["value", "enabled", "minimum_value", "maximum_value", "minimum_value_warning", "maximum_value_warning", "allow_empty", "is_uuid", "warning_value", "error_value"]
_STRING_PROPERTIES = ["type", "icon", "unit", "comments", "regex_blacklist_pattern"]
_ANY_PROPERTIES = ["label", "description", "warning_description", "error_description", "default_value", "options", "force_depends_on_settings"]
_DEFAULTS = {"label": "", "type": "", "icon": "", "unit": "", "description": "", "warning_description": "", "error_description": "",
             "default_value": 0, "value": None, "enabled": True, "minimum_value": None, "maximum_value": None, "minimum_value_warning": None,
             "maximum_value_warning": None, "options": {}, "comments": "", "allow_empty": True, "is_uuid": False, "regex_blacklist_pattern": "",
             "warning_value": None, "error_value": None, "force_depends_on_settings": []}


class SettingFunction:
    def __init__(self, expression):
        self._code = expression
        self._valid = False
        self._used_values = []
        try:
            self._used_values = _h.expr_names(self._code)
            self._valid = True
        except Exception as e:
            Logger.log("e", "Parse error in function ({1}) for setting: {0}".format(str(e), self._code))

    def __call__(self, value_provider, context=None, additional_variables=None):
        if not value_provider:
            return None
        if not self._valid:
            return None
        local_variables = {}
        for name in self._used_values:
            value = value_provider.getProperty(name, "value", context)
            if value is None:
                continue
            local_variables[name] = value
        if additional_variables is not None:
            local_variables.update(additional_variables)
        try:
            return _h.eval_setting(self._code, local_variables)
        except Exception as e:
            Logger.log("w", "An exception occurred in inherit function {0}: {1}".format(self, str(e)))
            return 0

    def __eq__(self, other):
        return isinstance(other, SettingFunction) and self._code == other._code

    def __str__(self):
        return "={0}".format(self._code)

    def __repr__(self):
        return "<UM.Settings.SettingFunction ={0} >".format(self._code)

    def isValid(self):
        return self._valid


class SettingDefinition:
    def __init__(self, key, container=None, parent=None, i18n_catalog=None):
        self._key = key
        self._container = container
        self._parent = parent
        self._children = []
        self._props = {}

    @property
    def key(self):
        return self._key

    @property
    def children(self):
        return self._children

    @property
    def parent(self):
        return self._parent

    def deserialize(self, d):
        self._children = []
        for key in d:
            value = d[key]
            if key == "children":
                for child_key in value:
                    child = SettingDefinition(child_key, self._container, self)
                    child.deserialize(value[child_key])
                    self._children.append(child)
                continue
            if key in _FUNCTION_PROPERTIES:
                self._props[key] = SettingFunction(str(value))
            elif key in _STRING_PROPERTIES:
                if key == "type" and value not in _FROM_STRING:
                    raise ValueError("Type {0} is not a correct setting type.".format(value))
                self._props[key] = str(value)
            elif key in _ANY_PROPERTIES:
                self._props[key] = value
            else:
                Logger.log("w", "Unrecognised property {0} in setting {1}".format(key, self._key))
        for req in ("label", "type", "description"):
            if req not in self._props:
                raise AttributeError("Setting {0} is missing required property {1}".format(self._key, req))

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        props = self.__dict__.get("_props", {})
        if name in props:
            return props[name]
        if name in _DEFAULTS:
            return _DEFAULTS[name]
        raise AttributeError("'SettingDefinition' object has no attribute '{0}'".format(name))

    def getAllKeys(self):
        keys = [self._key]
        for c in self._children:
            keys.extend(c.getAllKeys())
        return keys

    def toDict(self):
        out = {"key": self._key}
        for k in self._props:
            v = self._props[k]
            out[k] = str(v)[1:] if isinstance(v, SettingFunction) else v
        return out


class ContainerFormatError(Exception):
    pass


class DefinitionNotFoundError(Exception):
    pass


class DefinitionContainer:
    def __init__(self, container_id, i18n_catalog=None, parent=None, *args, **kwargs):
        self._id = str(container_id)
        self._metadata = {"id": self._id}
        self._definitions = []
        self._cache = {}
        self.propertyChanged = Signal()
        self.metaDataChanged = Signal()

    def getId(self):
        return self._id

    def getName(self):
        return self._metadata.get("name", self._id)

    def getMetaData(self):
        return self._metadata

    def getMetaDataEntry(self, entry, default=None):
        return self._metadata.get(entry, default)

    def setMetaDataEntry(self, key, value):
        self._metadata[key] = value

    def isReadOnly(self):
        return True

    def deserialize(self, serialized, file_name=None):
        try:
            parsed = json.loads(serialized)
        except Exception as e:
            raise ContainerFormatError("Unable to parse definition: " + str(e))
        if not isinstance(parsed, dict):
            raise ContainerFormatError("Definition is not a JSON object")
        for required in ("version", "name"):
            if required not in parsed:
                raise ContainerFormatError("Missing required property {0}".format(required))
        settings = parsed.pop("settings", None)
        self._metadata.update(parsed)
        self._definitions = []
        self._cache = {}
        if settings is not None:
            try:
                for key in settings:
                    definition = SettingDefinition(key, self, None)
                    definition.deserialize(settings[key])
                    self._definitions.append(definition)
            except (ValueError, AttributeError) as e:
                raise ContainerFormatError(str(e))
            for d in self._definitions:
                self._index(d)
        return serialized

    def _index(self, d):
        self._cache[d.key] = d
        for c in d.children:
            self._index(c)

    @property
    def definitions(self):
        return self._definitions

    def getAllKeys(self):
        return list(self._cache.keys())

    def findDefinitions(self, **kwargs):
        if "key" in kwargs and len(kwargs) == 1:
            d = self._cache.get(kwargs["key"])
            return [d] if d is not None else []
        out = []
        for d in self._cache.values():
            ok = True
            for k in kwargs:
                if getattr(d, k, None) != kwargs[k]:
                    ok = False
            if ok:
                out.append(d)
        return out

    def getProperty(self, key, property_name, context=None):
        d = self._cache.get(key)
        if d is None:
            return None
        try:
            value = getattr(d, property_name)
        except AttributeError:
            return None
        if value is None and property_name == "value":
            value = getattr(d, "default_value")
        return value

    def hasProperty(self, key, property_name, ignore_inherited=False):
        d = self._cache.get(key)
        return d is not None and property_name in d._props


class InstanceContainer:
    def __init__(self, container_id, parent=None, *args, **kwargs):
        self._id = str(container_id)
        self._metadata = {"id": self._id}
        self._definition_id = None
        self._values = {}
        self._states = {}
        self.propertyChanged = Signal()
        self.metaDataChanged = Signal()

    def getId(self):
        return self._id

    def getName(self):
        return self._metadata.get("name", self._id)

    def setDefinition(self, definition_id):
        self._definition_id = definition_id

    def getDefinition(self):
        found = ContainerRegistry.getInstance().findDefinitionContainers(id=self._definition_id)
        if not found:
            raise DefinitionNotFoundError("Could not find definition {0}".format(self._definition_id))
        return found[0]

    def getMetaData(self):
        return self._metadata

    def getMetaDataEntry(self, entry, default=None):
        return self._metadata.get(entry, default)

    def setMetaDataEntry(self, key, value):
        self._metadata[key] = value

    def setDirty(self, dirty):
        pass

    def isReadOnly(self):
        return False

    def getAllKeys(self):
        return set(self._values.keys())

    def setProperty(self, key, property_name, property_value, container=None, set_from_cache=False):
        if key not in self._values:
            try:
                definition = self.getDefinition()
            except DefinitionNotFoundError:
                Logger.log("w", "Tried to set value of setting %s when the InstanceContainer has no SettingDefinition.", key)
                return
            if not definition.findDefinitions(key=key):
                Logger.log("w", "Tried to set value of the setting %s, but it has no SettingInstance in this InstanceContainer %s or its SettingDefinition %s", key, self.getName(), definition.getName())
                return
        if property_name == "state":
            self._states[key] = property_value
            return
        if property_name != "value":
            return
        if isinstance(property_value, str) and property_value.strip().startswith("="):
            property_value = SettingFunction(property_value[1:])
        old = self._values.get(key)
        self._values[key] = property_value
        if old != property_value or key not in self._values:
            self.propertyChanged.emit(key, property_name)

    def getProperty(self, key, property_name, context=None):
        if key not in self._values:
            return None
        if property_name == "value":
            value = self._values[key]
            if isinstance(value, str):
                d = None
                try:
                    found = self.getDefinition().findDefinitions(key=key)
                    if found:
                        d = found[0]
                except DefinitionNotFoundError:
                    pass
                if d is not None:
                    try:
                        return settingValueFromString(d.type, value)
                    except Exception:
                        return value
            return value
        if property_name == "state":
            return self._states.get(key, "InstanceState.User")
        return None

    def hasProperty(self, key, property_name):
        return key in self._values and property_name in ("value", "state")

    def removeInstance(self, key, postpone_emit=False):
        if key in self._values:
            del self._values[key]

    def clear(self):
        self._values = {}


class ContainerStack:
    def __init__(self, stack_id, *args, **kwargs):
        self._id = str(stack_id)
        self._name = self._id
        self._containers = []
        self._metadata = {"id": self._id}
        self._next_stack = None
        self.propertyChanged = Signal()
        self.metaDataChanged = Signal()
        self.containersChanged = Signal()

    def getId(self):
        return self._id

    def getName(self):
        return self._name

    def setName(self, name):
        self._name = name

    def setDirty(self, dirty):
        pass

    def getMetaData(self):
        return self._metadata

    def getMetaDataEntry(self, entry, default=None):
        return self._metadata.get(entry, default)

    def setMetaDataEntry(self, key, value):
        self._metadata[key] = value

    def addContainer(self, container):
        self._containers.insert(0, container)
        if isinstance(container, InstanceContainer):
            container.propertyChanged.connect(self._containerPropertyChanged)

    def _containerPropertyChanged(self, key, property_name):
        self.propertyChanged.emit(key, property_name)

    def getContainers(self):
        return list(self._containers)

    def getContainer(self, index):
        return self._containers[index]

    def getTop(self):
        return self._containers[0] if self._containers else None

    def getBottom(self):
        return self._containers[-1] if self._containers else None

    def getNextStack(self):
        return self._next_stack

    def setNextStack(self, stack, connect_signals=True):
        self._next_stack = stack

    def getAllKeys(self):
        b = self.getBottom()
        return set(b.getAllKeys()) if b is not None else set()

    def getProperty(self, key, property_name, context=None):
        value = self.getRawProperty(key, property_name, context=context)
        if isinstance(value, SettingFunction):
            value = value(self, context)
        return value

    def getRawProperty(self, key, property_name, context=None, use_next=True, skip_until_container=None):
        containers = self._containers
        if property_name not in ["value", "state", "validationState"]:
            if containers:
                value = containers[-1].getProperty(key, property_name, context)
                if value is not None:
                    return value
        else:
            for container in containers:
                if skip_until_container and container.getId() != skip_until_container:
                    continue
                skip_until_container = None
                value = container.getProperty(key, property_name, context)
                if value is not None:
                    return value
        if self._next_stack and use_next:
            return self._next_stack.getRawProperty(key, property_name, context=context, use_next=use_next)
        return None

    def hasProperty(self, key, property_name):
        for c in self._containers:
            if c.hasProperty(key, property_name):
                return True
        return False

    def findContainer(self, criteria=None, container_type=None, **kwargs):
        for c in self._containers:
            if container_type is None or isinstance(c, container_type):
                return c
        return None


class ContainerRegistry:
    _instance = None

    def __init__(self):
        self._definitions = {}
        self._containers = {}
        self.containerAdded = Signal()
        self.containerRemoved = Signal()

    @classmethod
    def getInstance(cls):
        if cls._instance is None:
            cls._instance = ContainerRegistry()
        return cls._instance

    def findDefinitionContainers(self, **kwargs):
        if "id" in kwargs:
            d = self._definitions.get(kwargs["id"])
            return [d] if d is not None else []
        return list(self._definitions.values())

    def findContainerStacks(self, **kwargs):
        if "id" in kwargs:
            c = self._containers.get(kwargs["id"])
            return [c] if c is not None and isinstance(c, ContainerStack) else []
        return [c for c in self._containers.values() if isinstance(c, ContainerStack)]

    def findInstanceContainers(self, **kwargs):
        return []

    def findContainers(self, **kwargs):
        return self.findDefinitionContainers(**kwargs) + self.findContainerStacks(**kwargs)

    def addContainer(self, container):
        if isinstance(container, DefinitionContainer):
            self._definitions[container.getId()] = container
        else:
            self._containers[container.getId()] = container

    def removeContainer(self, container_id):
        self._containers.pop(container_id, None)
        self._definitions.pop(container_id, None)

    def isLoaded(self, container_id):
        return container_id in self._definitions or container_id in self._containers


# ---------------------------------------------------------------- Cura's application state

class _HostContainer:
    """A container (quality, material, variant, ...) of one of the printer's stacks."""

    def __init__(self, stack, kind):
        self._stack = stack
        self._kind = kind

    def getId(self):
        return _h.container_info(self._stack, self._kind, "id")

    def getName(self):
        return _h.container_info(self._stack, self._kind, "name")

    @property
    def id(self):
        return self.getId()

    @property
    def name(self):
        return self.getName()

    def getMetaData(self):
        return _h.container_metadata(self._stack, self._kind)

    def getMetaDataEntry(self, entry, default=None):
        v = _h.container_metadata(self._stack, self._kind).get(entry)
        return default if v is None else v

    def getProperty(self, key, property_name, context=None):
        return _h.container_property(self._stack, self._kind, key, property_name)

    def hasProperty(self, key, property_name):
        return _h.container_property(self._stack, self._kind, key, property_name) is not None

    def getAllKeys(self):
        return set(_h.container_keys(self._stack, self._kind))

    def isEmpty(self):
        return self.getId().startswith("empty")

    def __eq__(self, other):
        return isinstance(other, _HostContainer) and self._stack == other._stack and self._kind == other._kind


class _HostStack:
    """The global stack (index -1) or an extruder stack (index = position order)."""

    def __init__(self, index):
        self._index = index
        self.propertyChanged = Signal()
        self.metaDataChanged = Signal()
        self.containersChanged = Signal()

    def getProperty(self, key, property_name, context=None):
        return _h.stack_property(self._index, key, property_name)

    def getRawProperty(self, key, property_name, context=None, use_next=True, skip_until_container=None):
        return _h.stack_property(self._index, key, property_name)

    def hasProperty(self, key, property_name):
        return _h.stack_property(self._index, key, property_name) is not None

    def getValue(self, key):
        return self.getProperty(key, "value")

    def getId(self):
        return _h.container_info(self._index, "stack", "id")

    def getName(self):
        return _h.container_info(self._index, "stack", "name")

    @property
    def id(self):
        return self.getId()

    @property
    def name(self):
        return self.getName()

    def getMetaData(self):
        return _h.container_metadata(self._index, "stack")

    def getMetaDataEntry(self, entry, default=None):
        v = _h.container_metadata(self._index, "stack").get(entry)
        return default if v is None else v

    def setMetaDataEntry(self, key, value):
        _h.set_stack_metadata(self._index, key, value)
        self.metaDataChanged.emit(self)

    def getAllKeys(self):
        return set(_h.container_keys(self._index, "definition"))

    def getContainers(self):
        return [self.userChanges, self.qualityChanges, self.intent, self.quality, self.material, self.variant, self.definitionChanges, self.definition]

    def getTop(self):
        return self.userChanges

    def getBottom(self):
        return self.definition

    def getNextStack(self):
        if self._index >= 0:
            return _HostStack(-1)
        return None

    def isEnabled(self):
        if self._index < 0:
            return True
        return bool(_h.container_metadata(self._index, "stack").get("enabled", "True") != "False")

    @property
    def isEnabledProperty(self):
        return self.isEnabled()

    @property
    def userChanges(self):
        return _HostContainer(self._index, "userChanges")

    @property
    def qualityChanges(self):
        return _HostContainer(self._index, "qualityChanges")

    @property
    def intent(self):
        return _HostContainer(self._index, "intent")

    @property
    def quality(self):
        return _HostContainer(self._index, "quality")

    @property
    def material(self):
        return _HostContainer(self._index, "material")

    @property
    def variant(self):
        return _HostContainer(self._index, "variant")

    @property
    def definitionChanges(self):
        return _HostContainer(self._index, "definitionChanges")

    @property
    def definition(self):
        return _HostContainer(self._index, "definition")

    @property
    def extruderList(self):
        return [_HostStack(k) for k in range(_h.extruder_count())]

    @property
    def extruders(self):
        out = {}
        for k in range(_h.extruder_count()):
            out[str(k)] = _HostStack(k)
        return out

    def getExtruderPositionValueWithDefault(self, key):
        value = self.getProperty(key, "value")
        if value is None or int(value) < 0:
            return "0"
        return str(value)

    def __eq__(self, other):
        return isinstance(other, _HostStack) and self._index == other._index


class _PrintInformation:
    def _info(self):
        return _h.print_information()

    @property
    def jobName(self):
        return self._info()["jobName"]

    @property
    def baseName(self):
        return self._info()["jobName"]

    @property
    def currentPrintTime(self):
        return Duration(self._info()["printTime"])

    @property
    def printTimes(self):
        out = {}
        for k, v in self._info()["featureTimes"]:
            out[k] = Duration(v)
        return out

    @property
    def materialLengths(self):
        return self._info()["materialLengths"]

    @property
    def materialWeights(self):
        return self._info()["materialWeights"]

    @property
    def materialCosts(self):
        return self._info()["materialCosts"]

    @property
    def materialNames(self):
        return self._info()["materialNames"]

    def getFeaturePrintTimes(self):
        return self.printTimes


class _Preferences:
    def getValue(self, key):
        return _h.preference(key)

    def setValue(self, key, value):
        pass

    def addPreference(self, key, default):
        pass


class ExtruderManager:
    _instance = None

    @classmethod
    def getInstance(cls):
        if cls._instance is None:
            cls._instance = ExtruderManager()
        return cls._instance

    def getActiveExtruderStack(self):
        return _HostStack(_h.active_extruder())

    def getExtruderStack(self, index):
        index = int(index)
        if 0 <= index < _h.extruder_count():
            return _HostStack(index)
        return None

    def getActiveExtruderStacks(self):
        return [_HostStack(k) for k in range(_h.extruder_count())]

    def getMachineExtruders(self, machine_id=None):
        return self.getActiveExtruderStacks()

    def getUsedExtruderStacks(self):
        return [_HostStack(k) for k in _h.used_extruders()]

    def getFirstUsedExtruderStack(self):
        used = self.getUsedExtruderStacks()
        return sorted(used, key=lambda e: e.getValue("extruder_nr"))[0]

    def getInitialExtruderNr(self):
        return _h.initial_extruder()

    @property
    def activeExtruderIndex(self):
        return _h.active_extruder()

    @property
    def extruderIds(self):
        out = {}
        for k in range(_h.extruder_count()):
            out[str(k)] = _HostStack(k).getId()
        return out


class _OutputDeviceManager:
    def __init__(self):
        self.writeStarted = Signal()


class Application:
    _instance = None

    def __init__(self):
        self._global = _HostStack(-1)
        self.globalContainerStackChanged = Signal()
        self.mainWindowChanged = Signal()
        self._output = _OutputDeviceManager()

    @classmethod
    def getInstance(cls, *args, **kwargs):
        if Application._instance is None:
            Application._instance = Application()
        return Application._instance

    def getGlobalContainerStack(self):
        return self._global

    def getPrintInformation(self):
        return _PrintInformation()

    def getPreferences(self):
        return _Preferences()

    def getVersion(self):
        return _h.version()

    def getApplicationName(self):
        return "cura"

    def getApplicationDisplayName(self):
        return "UltiMaker Cura"

    def getExtruderManager(self):
        return ExtruderManager.getInstance()

    def getMachineManager(self):
        return None

    def getOutputDeviceManager(self):
        return self._output

    def getInstallPrefix(self):
        return ""

    def isQmlEngineInitialized(self):
        return False


CuraApplication = Application


# ---------------------------------------------------------------- cura.Snapshot / PyQt6 (thumbnails)

class QByteArray:
    def __init__(self, data=b""):
        self._data = bytes(data)

    def data(self):
        return self._data

    def __len__(self):
        return len(self._data)

    def toBase64(self):
        import base64
        return QByteArray(base64.b64encode(self._data))


class QIODevice:
    class OpenModeFlag:
        ReadOnly = 1
        WriteOnly = 2
        ReadWrite = 3


class QBuffer:
    OpenModeFlag = QIODevice.OpenModeFlag

    def __init__(self, *args):
        self._data = b""

    def open(self, mode):
        return True

    def close(self):
        pass

    def write(self, data):
        self._data = self._data + bytes(data)
        return len(data)

    def data(self):
        return self._data


class QImage:
    def __init__(self, png=b"", width=0, height=0):
        self._png = png
        self._width = width
        self._height = height

    def width(self):
        return self._width

    def height(self):
        return self._height

    def isNull(self):
        return not self._png

    def save(self, target, fmt="PNG", quality=-1):
        if isinstance(target, QBuffer):
            target.write(self._png)
            return True
        return False


class Snapshot:
    DEFAULT_WIDTH_HEIGHT = 300

    @staticmethod
    def snapshot(width=300, height=300, number_of_attempts=10):
        png = _h.snapshot(int(width), int(height))
        if png is None:
            Logger.log("w", "Unable to create snapshot as we seem to have an empty buildplate")
            return None
        return QImage(png, int(width), int(height))

    @staticmethod
    def isometricSnapshot(width=300, height=300, node=None):
        return Snapshot.snapshot(width, height)


# ---------------------------------------------------------------- the post-processing driver

_loaded_scripts = {}
_script_labels = {}


def _register(module, class_name):
    """PostProcessingPlugin.loadScripts for one loaded module."""
    loaded_class = getattr(module, class_name)
    temp_object = loaded_class()
    try:
        setting_data = temp_object.getSettingData()
        if "name" in setting_data and "key" in setting_data:
            _script_labels[setting_data["key"]] = setting_data["name"]
            _loaded_scripts[setting_data["key"]] = loaded_class
            return [setting_data["key"], setting_data["name"]]
        else:
            Logger.log("w", "Script %s.py has no name or key", class_name)
            _script_labels[class_name] = class_name
            _loaded_scripts[class_name] = loaded_class
            return [class_name, class_name]
    except AttributeError:
        Logger.log("e", "Script %s.py is not a recognised script type. Ensure it inherits Script", class_name)
    except NotImplementedError:
        Logger.log("e", "Script %s.py has no implemented settings", class_name)
    return None


def _definition_json(key):
    cls = _loaded_scripts[key]
    return json.dumps(cls().getSettingData())


def _create_script(key, settings):
    new_script = _loaded_scripts[key]()
    new_script.initialize()
    for setting_key, setting_value in settings:
        if new_script._instance is not None:
            new_script._instance.setProperty(setting_key, "value", setting_value)
    return new_script


def _script_state(key, settings):
    """Current values and UI state of each setting of a configured script."""
    script = _create_script(key, settings)
    data = script.getSettingData()
    out = []
    stack = script._stack
    for setting_key in data.get("settings", {}):
        value = script.getSettingValueByKey(setting_key)
        enabled = stack.getProperty(setting_key, "enabled") if stack is not None else True
        entry = {"key": setting_key, "value": value, "string": str(value), "enabled": bool(enabled)}
        for prop in ("minimum_value", "maximum_value", "minimum_value_warning", "maximum_value_warning"):
            try:
                v = stack.getProperty(setting_key, prop) if stack is not None else None
            except Exception:
                v = None
            if v is not None:
                entry[prop] = v
        out.append(entry)
    return out


def _serialize_settings(key, settings):
    """The (key, str(value)) pairs `writeScriptsToStack` stores for a script."""
    script = _create_script(key, settings)
    out = []
    for setting_key in script.getSettingData()["settings"]:
        value = script.getSettingValueByKey(setting_key)
        out.append((setting_key, str(value)))
    return out


def _execute(scripts, gcode_list):
    # Scripts are created (initialized) first, like Cura does when it restores them from the stack.
    instances = []
    for key, settings in scripts:
        if key not in _loaded_scripts:
            Logger.log("e", "Unknown post-processing script {script_name} was encountered in this global stack.".format(script_name=key))
            continue
        instances.append(_create_script(key, settings))
    for script in instances:
        try:
            gcode_list = script.execute(gcode_list)
        except Exception:
            Logger.logException("e", "Exception in post-processing script.")
    return gcode_list
