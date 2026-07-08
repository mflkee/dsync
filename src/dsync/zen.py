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
    m = re.search(r'^\[Install.+?\][\s\S]*?^Default\s*=\s*(.+)$', text, re.MULTILINE)
    if m:
        rel = m.group(1).strip()
        p = ZEN_CONFIG_DIR / rel
        if p.is_dir():
            return p

    # Fallback: find Default=1 profile
    m = re.search(r'^Default\s*=\s*1$[\s\S]*?^Path\s*=\s*(.+)$', text, re.MULTILINE)
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


def _normalize_url(url: str) -> str:
    """Normalize URL for dedup comparison."""
    url = url.rstrip("/")
    if url.startswith("https://"):
        return url
    if url.startswith("http://"):
        return url
    return url


def _tabs_contain(tabs: list, url: str) -> bool:
    url_norm = _normalize_url(url)
    for t in tabs:
        for e in t.get("entries", []):
            if _normalize_url(e.get("url", "")) == url_norm:
                return True
    return False


def _strip_tab(tab: dict) -> dict:
    """Remove bulky fields (images, storage, formdata) from a tab."""
    clean = {k: v for k, v in tab.items() if k not in ("image", "storage", "formdata", "_zenPinnedInitialState")}
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


def _merge_groups(local_groups: list, export_groups: list) -> list:
    """Merge groups by name. Add new groups, update matching ones."""
    merged = list(local_groups)
    local_by_name = {g.get("name"): g for g in merged if g.get("name")}

    for eg in export_groups:
        name = eg.get("name")
        if not name:
            continue
        if name in local_by_name:
            # Update existing group properties
            lg = local_by_name[name]
            for key in ("color", "pinned", "collapsed", "saveOnWindowClose"):
                if key in eg:
                    lg[key] = eg[key]
        else:
            # Add new group with new ID
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

    return merged


def _merge_folders(local_folders: list, export_folders: list,
                   group_id_map: dict | None = None) -> list:
    """Merge folders by name+workspaceId."""
    merged = list(local_folders)

    def _key(f):
        return (f.get("name"), f.get("workspaceId", ""))

    local_keys = {_key(f): i for i, f in enumerate(merged)}

    for ef in export_folders:
        k = _key(ef)
        name = ef.get("name")
        if not name:
            continue
        if k in local_keys:
            idx = local_keys[k]
            lf = merged[idx]
            for key in ("collapsed", "pinned", "userIcon", "saveOnWindowClose"):
                if key in ef:
                    lf[key] = ef[key]
        else:
            # Create new folder. IDs are machine-local, so generate new.
            new_id = str(uuid.uuid4().int)[:19]
            nf = {
                "id": new_id,
                "name": name,
                "workspaceId": ef.get("workspaceId", ""),
                "pinned": ef.get("pinned", True),
                "collapsed": ef.get("collapsed", False),
                "splitViewGroup": ef.get("splitViewGroup", False),
                "saveOnWindowClose": ef.get("saveOnWindowClose", True),
                "emptyTabIds": [],
            }
            for key in ("prevSiblingInfo", "parentId", "userIcon"):
                if key in ef:
                    nf[key] = ef[key]
            merged.append(nf)

    return merged


def _merge_spaces(local_spaces: list, export_spaces: list,
                  containers: dict | None = None) -> list:
    """Merge workspaces by name. Add new ones, update matching ones."""
    merged = list(local_spaces)
    local_by_name = {s.get("name"): s for s in merged if s.get("name")}

    for es in export_spaces:
        name = es.get("name")
        if not name:
            continue
        if name in local_by_name:
            # Update existing workspace properties
            ls = local_by_name[name]
            for key in ("icon", "theme", "hasCollapsedPinnedTabs"):
                if key in es:
                    ls[key] = es[key]
        else:
            # Generate new UUID and create workspace
            new_uuid = str(uuid.uuid4()).upper()
            new_workspace: dict = {
                "uuid": "{" + new_uuid + "}",
                "name": name,
                "icon": es.get("icon", "chrome://browser/skin/zen-icons/selectable/circle.svg"),
                "theme": es.get("theme", {"type": "gradient", "gradientColors": [], "opacity": 0.5, "texture": 0}),
                "hasCollapsedPinnedTabs": False,
            }
            # Try to find matching container
            if containers and es.get("containerTabId") is not None:
                exported_ctid = es["containerTabId"]
                # Look up the container by userContextId from containers.json
                for identity in containers.get("identities", []):
                    if identity.get("userContextId") == exported_ctid:
                        # The same container name exists locally, use its userContextId
                        # For standard containers, they match by l10nId
                        break
            merged.append(new_workspace)

    return merged


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
        if export.get("spaces") is not None:
            local["spaces"] = _merge_spaces(
                local.get("spaces", []),
                export["spaces"],
                export.get("containers"),
            )

        # Merge groups
        if export.get("groups") is not None:
            local["groups"] = _merge_groups(
                local.get("groups", []),
                export["groups"],
            )

        # Merge folders
        if export.get("folders") is not None:
            local["folders"] = _merge_folders(
                local.get("folders", []),
                export["folders"],
            )

        # Add new pinned tabs from export that don't exist locally
        # Build a set of all local URLs
        new_tabs_added = 0
        if export.get("pinned_tabs"):
            local_tabs = local.get("tabs", [])
            for pt in export["pinned_tabs"]:
                # Get the URL
                entries = pt.get("entries", [])
                if not entries:
                    continue
                url = entries[0].get("url", "")
                if not url or url == "about:blank":
                    continue
                # Check if we already have this URL
                if _tabs_contain(local_tabs, url):
                    continue
                # Create a new tab entry
                new_tab = {
                    "entries": [
                        {
                            "url": url,
                            "title": entries[0].get("title", ""),
                            "triggeringPrincipal_base64": "{}",
                        }
                    ],
                    "lastAccessed": 0,
                    "pinned": pt.get("pinned", True),
                    "hidden": False,
                    "index": len(local_tabs) + 1,
                    "userContextId": pt.get("userContextId", 0),
                    "attributes": {},
                }
                local_tabs.append(new_tab)
                new_tabs_added += 1

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
        new_sess: dict = {"lastCollected": 0, "tabs": [], "folders": [], "groups": [], "spaces": []}
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

    return True
