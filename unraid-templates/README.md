# unraid-templates

Unraid **Community Applications** templates for medi (Phase 5, `docs/.tasks/50`).

| File | Role |
|---|---|
| `medi.xml` | The CA container template. Pre-configures GPU passthrough (`/dev/dri`, NVIDIA env), the `/config` (RW) and `/media` (**Read-Only**) volumes, and the `8096` web-UI port. |
| `ca_profile.xml` | Maintainer profile CA scrapes for attribution + support links. |

The Community Applications plugin scrapes the repo to list the app in its store,
so these may also live in a **dedicated** GitHub repo (e.g. `michelve/unraid-templates`)
with `ca_profile.xml` at that repo's root. The `<Repository>` pulls the image from
GHCR: `ghcr.io/michelve/medi:latest` (publish `:latest` **and** version tags so users
can pin — `docs/.tasks/50` §Scaling).

## Install (per the task's verification)

1. Unraid → **Apps** (Community Applications) → gear/settings → add this repo's URL
   under *Template Repositories*.
2. Search **medi** → **Install**.
3. Pick the folders: Appdata → `/mnt/user/appdata/medi`, Media Library → your movies
   share (mounted **Read-Only**).
4. **Intel/AMD**: leave the `/dev/dri` device. **NVIDIA**: set *Extra Parameters* to
   `--runtime=nvidia` (Advanced view) and keep `NVIDIA_VISIBLE_DEVICES=all`.
5. Apply → the container starts hardware-accelerated with `/media` read-only.

## Hardware passthrough summary

| Host GPU | What the template sets |
|---|---|
| Intel (QSV) / AMD (VA-API) | `--device /dev/dri` |
| NVIDIA (NVENC) | `NVIDIA_VISIBLE_DEVICES=all` + *Extra Parameters* `--runtime=nvidia` |

## Icon

`medi-icon.png` (256×256) ships in this directory and both `medi.xml` and
`ca_profile.xml` reference it via its raw GitHub URL:
`https://raw.githubusercontent.com/michelve/medi/main/unraid-templates/medi-icon.png`.
That URL resolves once this repo is pushed to `main` (or adjust the path if the
templates move to a dedicated repo).
