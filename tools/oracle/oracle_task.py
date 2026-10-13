"""Dump what Cura 5.12's StartSliceJob would send to CuraEngine for a configured machine.
Input: JSON file path in env ORACLE_IN (list of jobs); output: JSON lines to ORACLE_OUT."""
import json, os, time

class FakeMsg:
    def __init__(self):
        self.repeated = {}
        self.msgs = {}
        self.fields = {}
    def addRepeatedMessage(self, name):
        m = FakeMsg(); self.repeated.setdefault(name, []).append(m); return m
    def getMessage(self, name):
        return self.msgs.setdefault(name, FakeMsg())
    def __setattr__(self, k, v):
        if k in ("repeated", "msgs", "fields"):
            object.__setattr__(self, k, v)
        else:
            self.fields[k] = v

def settings_list(msg):
    out = []
    for s in msg.repeated.get("settings", []):
        v = s.fields["value"]
        out.append([s.fields["name"], v.decode("utf-8") if isinstance(v, bytes) else v])
    return out

def run(app):
    import StartSliceJob as ssj_mod  # plugin module
    jobs = json.load(open(os.environ["ORACLE_IN"]))
    out = open(os.environ["ORACLE_OUT"], "w")
    from cura.Settings.CuraStackBuilder import CuraStackBuilder
    from UM.Settings.ContainerRegistry import ContainerRegistry
    mm = app.getMachineManager()
    em = app.getExtruderManager()
    if os.environ.get("ORACLE_ENGINE"):
        # Use a wrapper (e.g. CuraEngine with -m1 for a deterministic single-threaded slice).
        app.getPreferences().setValue("backend/location", os.environ["ORACLE_ENGINE"])
        app.getBackend().startEngine()
        for _ in range(50):
            app.processEvents(); time.sleep(0.02)
    ssj_mod.time.strftime = lambda fmt, *a: {"%H:%M:%S": "12:34:56", "%d-%m-%Y": "01-02-2026", "%w": "1"}.get(fmt, fmt)
    for i, job in enumerate(jobs):
        res = {"name": job.get("name", str(i))}
        try:
            machine = CuraStackBuilder.createMachine("oracle_%d" % i, job["machine"])
            if machine is None:
                raise RuntimeError("createMachine failed")
            mm.setActiveMachine(machine.getId())
            app.processEvents()
            for path in job.get("import_materials", []):
                # Preferences > Materials > Import.
                from cura.Settings.ContainerManager import ContainerManager
                res.setdefault("material_imports", []).append(ContainerManager.getInstance().importMaterialContainer(path))
                for _ in range(10):
                    app.processEvents(); time.sleep(0.01)
            for pos, ext in sorted(job.get("extruders", {}).items()):
                if "variant" in ext:
                    mm.setVariantByName(pos, ext["variant"])
                if "material" in ext:
                    mm.setMaterialById(pos, ext["material"])
                if "enabled" in ext:
                    mm.setExtruderEnabled(int(pos), ext["enabled"])
            if "quality_type" in job:
                mm.setQualityGroupByQualityType(job["quality_type"])
            if "intent" in job:
                mm.setIntentByCategory(job["intent"])
            gs = app.getGlobalContainerStack()
            for k, v in job.get("global", {}).items():
                gs.userChanges.setProperty(k, "value", v)
            for pos, ext in job.get("extruders", {}).items():
                for k, v in ext.get("user", {}).items():
                    gs.extruderList[int(pos)].userChanges.setProperty(k, "value", v)
            app.processEvents()
            if "custom_profile" in job:
                # Preferences > Profiles > Create (from current settings), then activate it.
                from cura.Machines.Models.QualityManagementModel import QualityManagementModel
                QualityManagementModel().createQualityChanges(job["custom_profile"], True)
                app.processEvents()
                res["quality_changes"] = [gs.qualityChanges.getId()] + [e.qualityChanges.getId() for e in gs.extruderList]
            if "export_profile" in job:
                # Preferences > Profiles > Export (ContainerManager.exportQualityChangesGroup).
                containers = [gs.qualityChanges] + [e.qualityChanges for e in gs.extruderList if e.qualityChanges.getId() != "empty_quality_changes"]
                res["export_ok"] = app.getContainerRegistry().exportQualityProfile(containers, job["export_profile"], "Cura Profile (*.curaprofile)")
            for path in job.get("import_profiles", []):
                r = app.getContainerRegistry().importProfile(path)
                app.processEvents()
                res.setdefault("import_results", []).append(r)
            if job.get("dump_profiles"):
                from cura.Machines.ContainerTree import ContainerTree
                qdef = ContainerTree.getInstance().machines[gs.definition.getId()].quality_definition
                reg = app.getContainerRegistry()
                groups = {}
                for md in reg.findContainersMetadata(type = "quality_changes", definition = qdef):
                    c = reg.findContainers(id = md["id"])[0]
                    groups.setdefault(c.getName(), {})[c.getId()] = c.serialize()
                res["profiles"] = groups
            res["containers"] = {
                "global": [c.getId() for c in gs.getContainers()],
                "extruders": [[c.getId() for c in e.getContainers()] for e in gs.extruderList],
            }
            # Load the models like File > Open does, and wait for them to land on the build plate.
            scene = app.getController().getScene()
            from UM.Scene.Iterator.DepthFirstIterator import DepthFirstIterator
            def sliceable():
                return [n for n in DepthFirstIterator(scene.getRoot()) if n.callDecoration("isSliceable")]
            for n in sliceable():
                n.getParent().removeChild(n)
            from PyQt6.QtCore import QUrl
            for mi, model in enumerate(job.get("models", [])):
                if "image" in model:
                    # The image import dialog, answered with the job's options.
                    o = model["image"]
                    ui = app.getPluginRegistry().getPluginObject("ImageReader")._ui
                    ui.showConfigUI = lambda: None
                    ui.waitForUIToClose = lambda: None
                    ui._cancelled = False
                    ui.peak_height = o.get("peak", 2.5)
                    ui.base_height = o.get("base", 0.4)
                    ui.smoothing = o.get("smoothing", 1)
                    ui.lighter_is_higher = o.get("lighter_is_higher", False)
                    ui.use_transparency_model = o.get("transparency", True)
                    ui.transmittance_1mm = o.get("transmittance", 50.0)
                    size = o.get("size", 120.0)
                    ui.getWidth = lambda size=size: size
                    ui.getDepth = lambda size=size: size
                before = len(sliceable())
                app.readLocalFile(QUrl.fromLocalFile(model["file"]))
                t0 = time.time()
                while len(sliceable()) <= before and time.time() - t0 < 30:
                    app.processEvents(); time.sleep(0.02)
                for _ in range(20):
                    app.processEvents(); time.sleep(0.01)
                node = sliceable()[-1]
                if "extruder" in model:
                    node.callDecoration("setActiveExtruder", gs.extruderList[int(model["extruder"])].getId())
                if "position" in model:
                    from UM.Math.Vector import Vector
                    x, y = model["position"]
                    node.setPosition(Vector(x, node.getPosition().y, -y))
                if "settings" in model:
                    # Like PerObjectSettingVisibilityHandler: create the instance from the stack's
                    # setting definition, then set the value.
                    from UM.Settings.SettingInstance import SettingInstance
                    stack = node.callDecoration("getStack")
                    top = stack.getTop()
                    for k, v in model["settings"].items():
                        if top.getInstance(k) is None:
                            inst = SettingInstance(stack.getSettingDefinition(k), top)
                            inst.setProperty("value", v)
                            inst.resetState()
                            top.addInstance(inst)
                        else:
                            top.setProperty(k, "value", v)
                app.processEvents()
            for _ in range(20):
                app.processEvents(); time.sleep(0.01)
            # Support blockers: SupportEraser._createEraserMesh(model, picked position).
            if any(m.get("support_blockers") for m in job.get("models", [])):
                from UM.Math.Vector import Vector
                eraser_tool = app.getPluginRegistry().getPluginObject("SupportEraser")
                models = [n for n in sliceable() if not n.callDecoration("getStack") or not n.callDecoration("getStack").getProperty("anti_overhang_mesh", "value")]
                for mi, model in enumerate(job.get("models", [])):
                    for b in model.get("support_blockers", []):
                        eraser_tool._createEraserMesh(models[mi], Vector(*b))
                        for _ in range(10):
                            app.processEvents(); time.sleep(0.01)
            res["nodes"] = [[n.getName(), list(n.getPosition().getData().tolist())] for n in sliceable()]
            msg = FakeMsg()
            job_obj = ssj_mod.StartSliceJob(msg)
            mec = app.getMachineErrorChecker()
            type(mec).needToWaitForResult = property(lambda self: False)
            res["has_error"] = mec.hasError
            type(mec).hasError = property(lambda self: False)
            job_obj.setBuildPlate(0)
            job_obj.run()
            res["result"] = int(job_obj.getResult())
            import hashlib, numpy
            res["objects"] = []
            for g in msg.repeated.get("object_lists", []):
                grp = {"settings": settings_list(g), "objects": []}
                for o in g.repeated.get("objects", []):
                    v = numpy.asarray(o.fields["vertices"], dtype=numpy.float32)
                    grp["objects"].append({"name": o.fields["name"], "n": int(v.shape[0]), "sha": hashlib.sha1(v.tobytes()).hexdigest(),
                                           "first": v[:3].tolist(), "min": v.min(axis=0).tolist(), "max": v.max(axis=0).tolist(),
                                           "settings": settings_list(o)})
                res["objects"].append(grp)
            res["global_settings"] = settings_list(msg.getMessage("global_settings"))
            res["limit_to_extruder"] = [[m.fields["name"], m.fields["extruder"]] for m in msg.repeated.get("limit_to_extruder", [])]
            res["extruders"] = [{"id": e.fields["id"], "settings": settings_list(e.getMessage("settings"))} for e in msg.repeated.get("extruders", [])]
            if job.get("gcode_out"):
                # Let Cura slice with its own CuraEngine and save the G-code like File > Export.
                from cura.CuraApplication import CuraApplication as _CA
                backend = app.getBackend()
                from UM.Backend.Backend import BackendState
                backend.forceSlice()
                t0 = time.time()
                state = None
                while time.time() - t0 < 600:
                    app.processEvents(); time.sleep(0.05)
                    state = backend._backend_state
                    if state in (BackendState.Done, BackendState.Error, BackendState.Disabled):
                        break
                for _ in range(10):
                    app.processEvents(); time.sleep(0.02)
                res["backend_state"] = int(state) if state is not None else None
                if job.get("dump_chunks"):
                    _scene = app.getController().getScene()
                    res["chunks"] = [len(c.encode("utf-8")) for c in getattr(_scene, "gcode_dict", {}).get(0, [])]
                if job.get("add_scripts") or job.get("post_processing") is not None:
                    # Post Processing Plugin: configure scripts like the dialog does, then let
                    # writeStarted run them (PostProcessingPlugin.execute).
                    pp = app.getPluginRegistry().getPluginObject("PostProcessingPlugin")
                    gs = app.getGlobalContainerStack()
                    pp.loadAllScripts()
                    if job.get("post_processing") is not None:
                        gs.setMetaDataEntry("post_processing_scripts", job["post_processing"])
                        pp._global_container_stack = gs
                        pp._restoreScriptInforFromMetadata()
                    else:
                        pp._global_container_stack = gs
                        pp._script_list.clear()
                    for key, settings in job.get("add_scripts", []):
                        pp.addScriptToList(key)
                        inst = pp._script_list[-1]._instance
                        for k, v in settings.items():
                            inst.setProperty(k, "value", v)
                    if job.get("add_scripts"):
                        pp.writeScriptsToStack()
                    res["pp_meta"] = gs.getMetaDataEntry("post_processing_scripts")
                    import UM.Message
                    shown = []
                    _orig_show = UM.Message.Message.show
                    UM.Message.Message.show = lambda self: shown.append([self._title, self._text])
                    pp.execute(None)
                    UM.Message.Message.show = _orig_show
                    res["pp_messages"] = shown
                import io
                writer = app.getPluginRegistry().getPluginObject("GCodeWriter")
                stream = io.StringIO()
                ok = writer.write(stream, [])
                res["gcode_ok"] = bool(ok)
                with open(job["gcode_out"], "w", newline="", encoding="utf-8") as f:
                    f.write(stream.getvalue())
                pi = app.getPrintInformation()
                res["print_time"] = pi.currentPrintTime.getDisplayString(3)
                res["material_lengths"] = list(pi.materialLengths)
                res["job_name"] = pi.jobName
        except Exception as e:
            import traceback
            res["error"] = traceback.format_exc()
        out.write(json.dumps(res) + "\n"); out.flush()
        try:
            mm.removeMachine(machine.getId())
        except Exception:
            pass
    out.close()
