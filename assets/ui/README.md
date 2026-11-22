# Desktop workspace assets

The workspace backdrops reproduce the atmospheric layers in the English
[light Overview](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=51-53176)
and [dark Overview](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=51-53567).
The PNGs let GPUI display the original colors as images.

From the repository root, regenerate the resources with librsvg.

```sh
rsvg-convert --width 1280 --height 800 assets/ui/workspace-light.svg -o assets/ui/workspace-light.png
rsvg-convert --width 1280 --height 800 assets/ui/workspace-dark.svg -o assets/ui/workspace-dark.png
rsvg-convert --width 64 --height 64 assets/icons/harbor/128.svg -o assets/ui/harbor.png
```
