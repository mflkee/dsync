"""Zen Browser profile export/import for dsync."""

import json
import re
import uuid
from pathlib import Path

import lz4.block

from . import ui

ZEN_CONFIG_DIR = Path.home() / ".config" / "zen"


def find_profile() -> Path | None:
    """Find the active Zen profile directory."""
    profiles_ini = ZEN_CONFIG_DIR / "profiles.ini"
    if not profiles_ini.exists():
        return None

    text = profiles_ini.read_text()

    # Find default profile from [Install*] section
    m = re.search(r"^\[Install.+?\][\s\S]*?^Default\s*=\s*(.+)$", text, re.MULTILINE)
    if m:
        rel = m.group(1).strip()
        p = ZEN_CONFIG_DIR / rel
        if p.is_dir():
            return p

    # Fallback: find Default=1 profile
    m = re.search(r"^Default\s*=\s*1$[\s\S]*?^Path\s*=\s*(.+)$", text, re.MULTILINE)
    if m:
        rel = m.group(1).strip()
        p = ZEN_CONFIG_DIR / rel
        if p.is_dir():
            return p

    return None


def _read_lz4(path: Path) -> dict:
    data = path.read_bytes()
    if data[:8] != b"mozLz40\0":
        raise ValueError(f"Not a mozlz4 file: {path}")
    raw = lz4.block.decompress(data[8:])
    return json.loads(raw)


def _write_lz4(path: Path, obj: dict):
    payload = json.dumps(obj, ensure_ascii=False).encode("utf-8")
    compressed = lz4.block.compress(payload)
    path.write_bytes(b"mozLz40\0" + compressed)


def _strip_tab(tab: dict) -> dict:
    """Remove bulky fields (images, storage, formdata) from a tab."""
    clean = {
        k: v
        for k, v in tab.items()
        if k not in ("image", "storage", "formdata", "_zenPinnedInitialState")
    }
    # Keep only the last entry URL/title for dedup
    entries = clean.get("entries", [])
    if entries:
        last = entries[-1]
        clean["entries"] = [{k: v for k, v in last.items() if k in ("url", "title")}]
    return clean


def export_zen(dest: Path) -> Path | None:
    """Export Zen profile data to a JSON file. Returns the path written, or None."""
    profile = find_profile()
    if profile is None:
        ui.print_error("Профиль Zen Browser не найден")
        return None

    ui.print_info(f"Профиль: {profile}")
    data: dict = {"_source": str(profile), "containers": None, "themes": None}

    # containers.json
    containers_path = profile / "containers.json"
    if containers_path.exists():
        data["containers"] = json.loads(containers_path.read_text())

    # zen-themes.json
    themes_path = profile / "zen-themes.json"
    if themes_path.exists():
        data["themes"] = json.loads(themes_path.read_text())

    # zen-sessions.jsonlz4
    session_path = profile / "zen-sessions.jsonlz4"
    if session_path.exists():
        sess = _read_lz4(session_path)
        data["spaces"] = sess.get("spaces", [])
        data["groups"] = sess.get("groups", [])
        data["folders"] = sess.get("folders", [])
        # Keep tabs only for pinned/essential info, strip bulky fields
        tabs = sess.get("tabs", [])
        pinned = [_strip_tab(t) for t in tabs if t.get("pinned")]
        data["pinned_tabs"] = pinned

    # zen-space-routing.jsonlz4
    routing_path = profile / "zen-space-routing.jsonlz4"
    if routing_path.exists():
        data["space_routing"] = _read_lz4(routing_path)

    # zen-live-folders.jsonlz4
    live_folders_path = profile / "zen-live-folders.jsonlz4"
    if live_folders_path.exists():
        data["live_folders"] = _read_lz4(live_folders_path)

    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(data, ensure_ascii=False, indent=2))
    return dest


def _merge_groups(local_groups: list, export_groups: list) -> tuple[list, dict]:
    """Merge groups by name. Add new groups, update matching ones.

    Returns (merged_groups, old_id_to_new_id_map).
    """
    merged = list(local_groups)
    local_by_name = {g.get("name"): g for g in merged if g.get("name")}
    id_map: dict[str, str] = {}

    for eg in export_groups:
        name = eg.get("name")
        old_id = eg.get("id", "")
        if not name:
            continue
        if name in local_by_name:
            # Update existing group properties
            lg = local_by_name[name]
            for key in ("color", "pinned", "collapsed", "saveOnWindowClose"):
                if key in eg:
                    lg[key] = eg[key]
            if old_id:
                id_map[old_id] = lg["id"]
        else:
            new_id = str(uuid.uuid4().int)[:19]
            new_group = {
                "id": new_id,
                "name": name,
                "color": eg.get("color", "zen-workspace-color"),
                "pinned": eg.get("pinned", True),
                "collapsed": eg.get("collapsed", False),
                "splitView": eg.get("splitView", False),
                "saveOnWindowClose": eg.get("saveOnWindowClose", True),
            }
            merged.append(new_group)
            if old_id:
                id_map[old_id] = new_id

    return merged, id_map


def _merge_folders(
    local_folders: list, export_folders: list, workspace_id_map: dict | None = None
) -> tuple[list, dict]:
    """Merge folders by name+workspaceId.

    Returns (merged_folders, old_id_to_new_id_map).
    """
    merged = list(local_folders)
    id_map: dict[str, str] = {}

    def _key(f):
        return (f.get("name"), f.get("workspaceId", ""))

    local_keys = {_key(f): i for i, f in enumerate(merged)}

    for ef in export_folders:
        k = _key(ef)
        name = ef.get("name")
        old_id = ef.get("id", "")
        if not name:
            continue
        # Translate workspaceId if needed
        ws_id = ef.get("workspaceId", "")
        if workspace_id_map and ws_id in workspace_id_map:
            ws_id = workspace_id_map[ws_id]
            k = (name, ws_id)
        if k in local_keys:
            idx = local_keys[k]
            lf = merged[idx]
            for key in ("collapsed", "pinned", "userIcon", "saveOnWindowClose"):
                if key in ef:
                    lf[key] = ef[key]
            if old_id:
                id_map[old_id] = lf["id"]
        else:
            new_id = str(uuid.uuid4().int)[:19]
            nf = {
                "id": new_id,
                "name": name,
                "workspaceId": ws_id,
                "pinned": ef.get("pinned", True),
                "collapsed": ef.get("collapsed", False),
                "splitViewGroup": ef.get("splitViewGroup", False),
                "saveOnWindowClose": ef.get("saveOnWindowClose", True),
                "emptyTabIds": [],
            }
            for key in ("prevSiblingInfo", "userIcon"):
                if key in ef:
                    nf[key] = ef[key]
            if "parentId" in ef and ef["parentId"] in id_map:
                nf["parentId"] = id_map[ef["parentId"]]
            elif "parentId" in ef:
                nf["parentId"] = ef["parentId"]
            merged.append(nf)
            if old_id:
                id_map[old_id] = new_id

    return merged, id_map


def _merge_spaces(
    local_spaces: list, export_spaces: list, containers: dict | None = None
) -> tuple[list, dict]:
    """Merge workspaces by name. Add new ones, update matching ones.

    Returns (merged_spaces, old_uuid_to_new_uuid_map).
    """
    merged = list(local_spaces)
    local_by_name = {s.get("name"): s for s in merged if s.get("name")}
    uuid_map: dict[str, str] = {}

    for es in export_spaces:
        name = es.get("name")
        old_uuid = es.get("uuid", "")
        if not name:
            continue
        if name in local_by_name:
            ls = local_by_name[name]
            for key in ("icon", "theme", "hasCollapsedPinnedTabs"):
                if key in es:
                    ls[key] = es[key]
            if old_uuid:
                uuid_map[old_uuid] = ls["uuid"]
        else:
            new_uuid = "{" + str(uuid.uuid4()).upper() + "}"
            new_workspace: dict = {
                "uuid": new_uuid,
                "name": name,
                "icon": es.get(
                    "icon", "chrome://browser/skin/zen-icons/selectable/circle.svg"
                ),
                "theme": es.get(
                    "theme",
                    {
                        "type": "gradient",
                        "gradientColors": [],
                        "opacity": 0.5,
                        "texture": 0,
                    },
                ),
                "hasCollapsedPinnedTabs": False,
            }
            if containers and es.get("containerTabId") is not None:
                exported_ctid = es["containerTabId"]
                for identity in containers.get("identities", []):
                    if identity.get("userContextId") == exported_ctid:
                        break
            merged.append(new_workspace)
            if old_uuid:
                uuid_map[old_uuid] = new_uuid

    return merged, uuid_map


def import_zen(source: Path) -> bool:
    """Import Zen data from a JSON file into the local profile."""
    if not source.exists():
        ui.print_error(f"Файл не найден: {source}")
        return False

    profile = find_profile()
    if profile is None:
        ui.print_error("Профиль Zen Browser не найден")
        return False

    export = json.loads(source.read_text())

    # containers.json - write directly
    if export.get("containers") is not None:
        (profile / "containers.json").write_text(
            json.dumps(export["containers"], ensure_ascii=False, indent=2)
        )
        ui.print_ok("containers.json — обновлён")

    # zen-themes.json - write directly
    if export.get("themes") is not None:
        (profile / "zen-themes.json").write_text(
            json.dumps(export["themes"], ensure_ascii=False, indent=2)
        )
        ui.print_ok("zen-themes.json — обновлён")

    # zen-sessions.jsonlz4 - merge
    session_path = profile / "zen-sessions.jsonlz4"
    if session_path.exists():
        local = _read_lz4(session_path)

        old_spaces_count = len(local.get("spaces", []))
        old_groups_count = len(local.get("groups", []))
        old_folders_count = len(local.get("folders", []))

        # Merge spaces
        workspace_uuid_map: dict[str, str] = {}
        if export.get("spaces") is not None:
            merged, wmap = _merge_spaces(
                local.get("spaces", []),
                export["spaces"],
                export.get("containers"),
            )
            local["spaces"] = merged
            workspace_uuid_map = wmap

        # Merge groups
        group_id_map: dict[str, str] = {}
        if export.get("groups") is not None:
            merged, gmap = _merge_groups(
                local.get("groups", []),
                export["groups"],
            )
            local["groups"] = merged
            group_id_map = gmap

        # Merge folders
        folder_id_map: dict[str, str] = {}
        if export.get("folders") is not None:
            merged, fmap = _merge_folders(
                local.get("folders", []),
                export["folders"],
                workspace_uuid_map,
            )
            local["folders"] = merged
            folder_id_map = fmap

        # Replace pinned tabs with exported ones (with ID translation)
        if export.get("pinned_tabs"):
            local_tabs = local.get("tabs", [])
            new_pinned: list[dict] = []
            for pt in export["pinned_tabs"]:
                entries = pt.get("entries", [])
                if not entries:
                    continue
                url = entries[0].get("url", "")
                if not url or url == "about:blank":
                    continue

                group_id = pt.get("groupId", "")
                if group_id in group_id_map:
                    group_id = group_id_map[group_id]

                zen_workspace = pt.get("zenWorkspace", "")
                if zen_workspace in workspace_uuid_map:
                    zen_workspace = workspace_uuid_map[zen_workspace]

                folder_id = pt.get("zenLiveFolderItemId", "")
                if folder_id in folder_id_map:
                    folder_id = folder_id_map[folder_id]
                elif folder_id:
                    folder_id = None

                new_tab = {
                    "entries": [
                        {
                            "url": url,
                            "title": entries[0].get("title", ""),
                            "triggeringPrincipal_base64": "{}",
                        }
                    ],
                    "lastAccessed": 0,
                    "pinned": True,
                    "hidden": False,
                    "index": len(new_pinned) + 1,
                    "groupId": group_id or None,
                    "zenWorkspace": zen_workspace or None,
                    "zenLiveFolderItemId": folder_id,
                    "zenSyncId": pt.get("zenSyncId", ""),
                    "userContextId": pt.get("userContextId", 0),
                    "attributes": {},
                }
                new_pinned.append(new_tab)

            # Keep only non-pinned tabs, replace pinned with exported
            local["tabs"] = [t for t in local_tabs if not t.get("pinned")] + new_pinned
            new_tabs_added = len(new_pinned)

            local["tabs"] = local_tabs

        if export.get("live_folders") is not None:
            local.setdefault("liveFolders", [])
            # Merge live folders by checking if they exist
            existing_ids = {f.get("id") for f in local["liveFolders"]}
            for lf in export["live_folders"]:
                if lf.get("id") not in existing_ids:
                    local["liveFolders"].append(lf)

        _write_lz4(session_path, local)

        new_spaces = len(local.get("spaces", [])) - old_spaces_count
        new_groups = len(local.get("groups", [])) - old_groups_count
        new_folders = len(local.get("folders", [])) - old_folders_count
        total_tabs = len(local.get("tabs", []))

        ui.print_ok("zen-sessions.jsonlz4 — обновлён")
        if new_spaces > 0:
            ui.print_info(f"  Добавлено рабочих пространств: {new_spaces}")
        if new_groups > 0:
            ui.print_info(f"  Добавлено групп: {new_groups}")
        if new_folders > 0:
            ui.print_info(f"  Добавлено папок: {new_folders}")
        if new_tabs_added > 0:
            ui.print_info(f"  Добавлено вкладок: {new_tabs_added}")
        ui.print_info(f"  Всего вкладок: {total_tabs}")
    else:
        ui.print_warn("zen-sessions.jsonlz4 не найден — создаю новый")
        new_sess: dict = {
            "lastCollected": 0,
            "tabs": [],
            "folders": [],
            "groups": [],
            "spaces": [],
        }
        if export.get("spaces"):
            new_sess["spaces"] = export["spaces"]
        if export.get("groups"):
            new_sess["groups"] = export["groups"]
        if export.get("folders"):
            new_sess["folders"] = export["folders"]
        _write_lz4(session_path, new_sess)
        ui.print_ok("zen-sessions.jsonlz4 — создан")

    # zen-space-routing.jsonlz4 - write directly
    if export.get("space_routing") is not None:
        _write_lz4(profile / "zen-space-routing.jsonlz4", export["space_routing"])
        ui.print_ok("zen-space-routing.jsonlz4 — обновлён")

    # zen-live-folders.jsonlz4 - write directly
    if export.get("live_folders") is not None:
        _write_lz4(profile / "zen-live-folders.jsonlz4", export["live_folders"])
        ui.print_ok("zen-live-folders.jsonlz4 — обновлён")

    _clean_sessionstore(profile)
    return True


def _clean_sessionstore(profile: Path) -> None:
    """Remove Firefox sessionstore files so Zen uses the imported session."""
    sessionstore = profile / "sessionstore.jsonlz4"
    if sessionstore.exists():
        sessionstore.unlink()
        ui.print_info("sessionstore.jsonlz4 — удалён (восстановление сессии отключено)")

    backups = profile / "sessionstore-backups"
    if backups.is_dir():
        import shutil
        shutil.rmtree(backups)
        ui.print_info("sessionstore-backups/ — удалены")
