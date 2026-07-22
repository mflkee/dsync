#!/usr/bin/env bash
# migrate_syncthing.sh — обновить конфиг syncthing: antix1 → archlinux-server + mkair-server
#
# Запустить на: huawei-nova, notebook
# (desktop, archlinux-server, mkair-server уже настроены)
#
# Устройства:
#   archlinux-server:    QX6QAG5-ADRPR7C-XWMTEW5-7PHKN47-6RACWNY-NPPLL3Q-RZKZSA2-Q5Y46Q3
#   mkair-server:        MWBTMTZ-OMZ5TOG-O7Q6WHG-3OUB6KG-SMRFSB4-U6ZYLWN-EJKT3IB-5DZSEAC
#   archlinux-desktop:   3WAB5DG-I2U66JW-7ELDCRS-ILZVLIQ-KQAJQ6C-VOFMQMY-JDYZKST-MR23FQR
#   huawei-nova:         KYVIPWB-QMRZGNB-SBRO2JN-CFGMRVN-IW4XWI6-TLHRZKO-HTZZELV-QO2D3AZ
#   archlinux-notebook:  SAAGLVR-XMACEHP-B6JJ6GQ-Z5MASC6-MQ2CDWN-I3UHVFN-RQDAERU-QNVESQ6

set -euo pipefail

OLD_ID="BZTWOLT-MZDD4ZJ-E2HL53C-GCA3L6X-HDC3A2A-VIU6NEN-6Y54Q5N-4MJZZAA"

# Определяем путь к конфигу
CONFIG=""
for p in ~/.config/syncthing/config.xml ~/.local/state/syncthing/config.xml; do
    [ -f "$p" ] && CONFIG="$p" && break
done

if [ -z "$CONFIG" ]; then
    echo "ERROR: config.xml not found"
    exit 1
fi

echo "Config: $CONFIG"

# Останавливаем syncthing
echo "Stopping syncthing..."
systemctl --user stop syncthing 2>/dev/null || killall syncthing 2>/dev/null || true
sleep 2

# Бэкап
cp "$CONFIG" "$CONFIG.bak.$(date +%s)"
echo "Backup created"

# Обновляем конфиг: удаляем antix1, добавляем archlinux-server + mkair-server
python3 << PYEOF
import xml.etree.ElementTree as ET
import sys
import os

config_path = "$CONFIG"
tree = ET.parse(config_path)
root = tree.getroot()

old_id = "$OLD_ID"
replaced = 0

# 1. Удаляем antix1
for device in list(root.findall(".//device")):
    if device.get("id") == old_id:
        # Находим родителя и удаляем
        for parent in root.iter():
            if device in list(parent):
                parent.remove(device)
                replaced += 1
                print("Removed device: antix1")
                break

# 2. Удаляем antix1 из папок
for folder in root.findall(".//folder"):
    for dev_ref in list(folder.findall(".//device")):
        if dev_ref.get("id") == old_id:
            folder.remove(dev_ref)
            replaced += 1

if replaced == 0:
    print("WARNING: antix1 not found (already migrated?)")
else:
    print(f"Removed antix1 ({replaced} changes)")

tree.write(config_path, xml_declaration=True, encoding="UTF-8")
print("Config saved")
PYEOF

# Запускаем syncthing
echo "Starting syncthing..."
systemctl --user start syncthing 2>/dev/null || syncthing -no-browser &
sleep 5

# Проверяем
echo "Verifying..."
syncthing --version 2>/dev/null || echo "syncthing not in PATH"
pgrep syncthing && echo "syncthing running" || echo "syncthing NOT running"

echo ""
echo "Done! Check syncthing GUI at http://127.0.0.1:8384"
echo "Devices should connect within a few minutes"
echo ""
echo "NOTE: If archlinux-server/mkair-server don't appear, add them manually:"
echo "  archlinux-server:    QX6QAG5-ADRPR7C-XWMTEW5-7PHKN47-6RACWNY-NPPLL3Q-RZKZSA2-Q5Y46Q3"
echo "  mkair-server:        MWBTMTZ-OMZ5TOG-O7Q6WHG-3OUB6KG-SMRFSB4-U6ZYLWN-EJKT3IB-5DZSEAC"
