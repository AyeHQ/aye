import os
import sysconfig


def find_aye_bin() -> str:
    """Return the path to the aye binary installed alongside this package."""
    exe = "aye" + (sysconfig.get_config_var("EXE") or "")

    candidates = [sysconfig.get_path("scripts")]

    # The prefix this package was installed into, which differs from the
    # running interpreter's in layered environments (`uv run --with`):
    # <prefix>/lib/pythonX.Y/site-packages/aye, or <prefix>/Lib/site-packages/aye.
    package = os.path.dirname(os.path.abspath(__file__))
    site_packages = os.path.dirname(package)
    if os.name == "nt":
        candidates.append(os.path.join(os.path.dirname(os.path.dirname(site_packages)), "Scripts"))
    else:
        candidates.append(
            os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(site_packages))), "bin")
        )

    candidates.append(sysconfig.get_path("scripts", scheme=sysconfig.get_preferred_scheme("user")))
    # `--target` installs: the binary sits in a sibling bin directory.
    candidates.append(os.path.join(site_packages, "bin"))

    for directory in candidates:
        path = os.path.join(directory, exe)
        if os.path.isfile(path):
            return path
    raise FileNotFoundError(f"could not find the aye binary; looked in {candidates}")
