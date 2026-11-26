"""Launch an installed Android Studio and Docker plugin in a fresh disposable project."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from xml.etree import ElementTree


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("socket", type=Path)
    parser.add_argument("plugin", type=Path, help="extracted clouds-docker-impl directory")
    parser.add_argument("--app", type=Path, default=Path("/Applications/Android Studio.app"))
    args = parser.parse_args()
    if not args.socket.is_absolute() or not args.socket.is_socket():
        parser.error("expected an absolute Unix socket path")
    if not (args.plugin / "lib/clouds-docker-impl.jar").is_file():
        parser.error("select the extracted official JetBrains Docker plugin")
    config = Path(os.environ["DOCKER_CONFIG"])
    if not config.is_absolute() or not config.is_dir():
        parser.error("select the private Docker fixture configuration with DOCKER_CONFIG")
    contents = args.app.resolve() / "Contents"
    with tempfile.TemporaryDirectory(prefix="marsa-ide-", dir="/private/tmp") as tmp:
        root = Path(tmp)
        for directory in ("config/options", "system", "log", "plugins", "home", "android", "project/.idea"):
            (root / directory).mkdir(parents=True)
        shutil.copytree(args.plugin, root / "plugins/clouds-docker-impl")
        properties = root / "profile.properties"
        properties.write_text("".join(f"idea.{part}.path={root / part}\n"
                                      for part in ("config", "system", "log", "plugins")))
        vmoptions = root / "studio.vmoptions"
        vmoptions.write_text((contents / "bin/studio.vmoptions").read_text()
                             + f"\n-Duser.home={root / 'home'}\n-Didea.properties.file={properties}\n")
        endpoint = f"unix://{args.socket}"
        options = ElementTree.Element("application")
        servers = ElementTree.SubElement(options, "component", name="RemoteServers")
        server = ElementTree.SubElement(servers, "remote-server", name="Marsa G4", type="docker")
        settings = ElementTree.SubElement(server, "configuration")
        ElementTree.SubElement(settings, "option", name="apiUrl", value=endpoint)
        ElementTree.ElementTree(options).write(root / "config/options/remote-servers.xml", encoding="utf-8")
        (root / "project/.idea/.name").write_text("Marsa Docker Qualification\n")
        env = {k: v for k, v in os.environ.items() if not k.startswith(("DOCKER_", "STUDIO_", "ANDROID_"))}
        env.update(HOME=str(root / "home"), ANDROID_USER_HOME=str(root / "android"),
                   STUDIO_PROPERTIES=str(properties), STUDIO_VM_OPTIONS=str(vmoptions),
                   DOCKER_HOST=endpoint, DOCKER_CONFIG=str(config))
        subprocess.run([contents / "MacOS/studio", root / "project"], env=env, check=True)


if __name__ == "__main__":
    main()
