# Harbor macOS app icon

`Marsa.icns` is built from the [Marsa Brand production icon artwork](https://www.figma.com/design/iXC1vWPKBgY2B6kDQVxI9W/Marsa-Desktop?node-id=2-17384). The `harbor` directory contains the original self-contained full, small and tiny vector exports. The application bundle uses `Contents/Resources/Marsa.icns` and sets `CFBundleIconFile` to `Marsa.icns`. The separate menu-bar mark belongs to the desktop implementation.

| Pixel size | Figma node | Art |
|---|---|---|
| 1024 | `2:17387` | Full |
| 512 | `2:17393` | Full |
| 128 | `2:17399` | Full |
| 32 | `2:17405` | Small |
| 16 | `2:17411` | Tiny |

Build from this directory using librsvg 2.63.4 and macOS `iconutil`. The 64 and 256 pixel renditions scale the full vector artwork. The 16 and 32 pixel renditions use the distinct designs. Run the commands in a scratch directory containing the checked-in `harbor` source files.

```sh
mkdir Marsa.iconset
render() {
    rsvg-convert --width "$2" --height "$2" "harbor/$1.svg" -o "Marsa.iconset/$3.png"
}
render 16 16 icon_16x16
render 32 32 icon_32x32
cp Marsa.iconset/icon_32x32.png Marsa.iconset/icon_16x16@2x.png
render 128 64 icon_32x32@2x
render 128 128 icon_128x128
render 512 256 icon_256x256
cp Marsa.iconset/icon_256x256.png Marsa.iconset/icon_128x128@2x.png
render 512 512 icon_512x512
cp Marsa.iconset/icon_512x512.png Marsa.iconset/icon_256x256@2x.png
render 1024 1024 icon_512x512@2x
iconutil -c icns -o Marsa.icns Marsa.iconset
```

Two builds produced byte-identical resources on macOS 27.2.1. ImageIO decoded ten representations from 16 to 1024 pixels. The rendered corners were transparent, and AppKit resolved the custom icon from an isolated bundle with the configured resource name. Run `shasum -a 256 -c SHA256SUMS` before using this asset in the release bundle. [Apple bundle icon key](https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundleiconfile)
