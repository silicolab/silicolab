"""Write Finder metadata directly, without UI automation on CI runners."""
from ds_store import DSStore
from mac_alias import Alias

WINDOW = {
    "WindowBounds": "{{200, 180}, {640, 360}}",
    "ShowSidebar": False,
    "ShowToolbar": False,
    "ShowStatusBar": False,
    "ShowPathbar": False,
    "ContainerShowSidebar": False,
    "SidebarWidth": 0,
}
ICON_VIEW = {
    "viewOptionsVersion": 1,
    "backgroundType": 2,
    "backgroundColorRed": 1.0,
    "backgroundColorGreen": 1.0,
    "backgroundColorBlue": 1.0,
    "scrollPositionX": 0.0,
    "scrollPositionY": 0.0,
    "iconSize": 160.0,
    "textSize": 14.0,
    "gridSpacing": 100.0,
    "gridOffsetX": 0.0,
    "gridOffsetY": 0.0,
    "arrangeBy": "none",
    "labelOnBottom": True,
    "showIconPreview": True,
    "showItemInfo": False,
}
POSITIONS = {"SilicoLab.app": (160, 160), "Applications": (480, 160)}


def background_alias(directory):
    alias = Alias.for_file(str((directory / ".background.tiff").resolve()))
    # Keep the volume/file identity but remove the temporary build mount path.
    alias.volume.posix_path = "/Volumes/SilicoLab"
    alias.target.folder_name = "SilicoLab"
    return alias.to_bytes()


def write_layout(directory):
    icons = dict(ICON_VIEW, backgroundImageAlias=background_alias(directory))
    with DSStore.open(str(directory / ".DS_Store"), "w+") as store:
        store["."]["vSrn"] = ("long", 1)
        store["."]["icvl"] = ("type", b"icnv")
        store["."]["bwsp"] = WINDOW
        store["."]["icvp"] = icons
        for name, position in POSITIONS.items():
            store[name]["Iloc"] = position


def validate_layout(directory):
    with DSStore.open(str(directory / ".DS_Store"), "r") as store:
        if (store["."]["vSrn"] != (b"long", 1)
                or store["."]["icvl"] != (b"type", b"icnv")):
            raise ValueError("DMG must open in Finder icon view")
        icons = store["."]["icvp"]
        alias = Alias.from_bytes(icons.pop("backgroundImageAlias"))
        if (alias.volume.posix_path != "/Volumes/SilicoLab"
                or alias.target.posix_path != "/.background.tiff"
                or not (directory / ".background.tiff").is_file()):
            raise ValueError("DMG arrow background must resolve inside its own volume")
        if store["."]["bwsp"] != WINDOW or icons != ICON_VIEW:
            raise ValueError("DMG window or large-icon settings do not match the installer layout")
        for name, position in POSITIONS.items():
            if store[name]["Iloc"] != position:
                raise ValueError(f"Wrong Finder icon position for {name}")
    return {"window_bounds": WINDOW["WindowBounds"], "icon_size": ICON_VIEW["iconSize"],
            "positions": POSITIONS, "background": ".background.tiff"}
