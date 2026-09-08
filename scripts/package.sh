#!/usr/bin/env bash
set -euo pipefail

# Package a release binary built for the supplied Rust target.
release_target=${1:?usage: scripts/package.sh RUST_TARGET}
release_version=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')
if [[ ! $release_version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo 'Package version must be a stable major.minor.patch version' >&2
    exit 1
fi
release_binary="$PWD/target/$release_target/release/remodeco"
release_name="remodeco-$release_version-$release_target"
release_work=$(mktemp -d)
trap 'rm -rf "$release_work"' EXIT
mkdir -p dist "$release_work/$release_name"
install -m 755 "$release_binary" "$release_work/$release_name/remodeco"
cp LICENSE README.md ARCHITECTURE.md "$release_work/$release_name/"
tar -czf "dist/$release_name.tar.gz" -C "$release_work" "$release_name"

case "$release_target" in
    x86_64-unknown-linux-musl) deb_arch=amd64; rpm_arch=x86_64 ;;
    aarch64-unknown-linux-musl) deb_arch=arm64; rpm_arch=aarch64 ;;
    *-apple-darwin)
        mkdir -p "$release_work/pkg/usr/local/bin"
        install -m 755 "$release_binary" "$release_work/pkg/usr/local/bin/remodeco"
        pkgbuild --root "$release_work/pkg" --identifier net.panta.remodeco \
            --version "$release_version" --install-location / \
            "dist/$release_name.pkg"
        exit 0
        ;;
    *) echo "Unsupported release target: $release_target" >&2; exit 1 ;;
esac

# Static musl executables require no distribution-specific shared libraries.
if readelf -l "$release_binary" | grep -q INTERP; then
    echo 'Linux packages require a statically linked executable' >&2
    exit 1
fi
mkdir -p "$release_work/deb/DEBIAN" "$release_work/deb/usr/bin" \
    "$release_work/deb/usr/share/doc/remodeco"
install -m 755 "$release_binary" "$release_work/deb/usr/bin/remodeco"
cp LICENSE README.md ARCHITECTURE.md "$release_work/deb/usr/share/doc/remodeco/"
cat > "$release_work/deb/DEBIAN/control" <<CONTROL
Package: remodeco
Version: $release_version
Section: utils
Priority: optional
Architecture: $deb_arch
Maintainer: Ivan Pantic <panta82@users.noreply.github.com>
Homepage: https://github.com/panta82/remodeco
Description: Review and execute file renames in your editor
 Full-screen terminal preview for file moves, copies, and desktop trash,
 with persistent sessions and undo journals.
CONTROL
dpkg-deb --root-owner-group --build "$release_work/deb" "dist/remodeco_${release_version}_${deb_arch}.deb"

mkdir -p "$release_work/rpm/SOURCES"
cp "$release_binary" LICENSE README.md ARCHITECTURE.md "$release_work/rpm/SOURCES/"
rpmbuild -bb packaging/remodeco.spec \
    --define "_topdir $release_work/rpm" \
    --define "release_version $release_version" \
    --target "$rpm_arch"
cp "$release_work/rpm/RPMS/$rpm_arch/"*.rpm dist/
