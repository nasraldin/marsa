# Desktop workspace assets

The workspace backdrops reproduce the atmospheric layers in the English
[light Overview](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=51-53176)
and [dark Overview](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=51-53567).
The PNGs let GPUI display the original colors as images.

Brand marks (`docker.svg`, `docker-stack.svg`, `podman.svg`, `argocd.svg`,
`incus.svg`, `k8s.svg`, `watchtower.svg`, `mcp.png`) keep their authored
colors. The GUI loads them with `img(...)` and must not tint them with
`text_color` or `Icon`.

From the repository root, regenerate the resources with librsvg.

```sh
rsvg-convert --width 1280 --height 800 assets/ui/workspace-light.svg -o assets/ui/workspace-light.png
rsvg-convert --width 1280 --height 800 assets/ui/workspace-dark.svg -o assets/ui/workspace-dark.png
rsvg-convert --width 64 --height 64 assets/icons/harbor/128.svg -o assets/ui/harbor.png
```
