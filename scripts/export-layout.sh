#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Export one installation set or firmware package as plain files without
# unemu: the layout `unemu image export` writes (backlog/docs/commands.md,
# Exporting an image; backlog/docs/design/recipes.md §3.10; decision-37).
#
# It reads an OCI Image Layout: the images folder of a store (STORE/images),
# or what any OCI client saves, such as `oras copy --to-oci-layout`. It needs
# a POSIX shell, jq, sha256sum, zstd and a UTF-8 locale; --files needs
# mtools too, which runs with MTOOLSRC=/dev/null. Every blob is checked
# against its digest, and a decoded layer against its unemu.decoded.digest;
# the names are checked as unemu checks them before anything is written.
#
# It writes the same files as `unemu image export REF --to DIR [--files]`:
# each layer under its org.opencontainers.image.title (without .zst, after
# zstd -d, for a +zstd layer), config.toml and manifest.json as stored,
# EXPORT-VERSION, and SHA256SUMS over them. With --files, the folder files
# holds the trees of the set's diskettes, merged in the set's order, with
# their entries' times taken as UTC (mcopy -m with TZ=UTC0); as with unemu,
# a diskette that cannot be read is left out with a `not read` line on
# standard output. Unlike unemu it writes only into an empty or absent DIR.
#
# Usage: scripts/export-layout.sh [--files] LAYOUT REF DIR
#        scripts/export-layout.sh --help
# REF is the image's org.opencontainers.image.ref.name in LAYOUT/index.json
# (a store names it NAME:TAG), or its manifest digest sha256:HEX.
set -eu

usage() {
	sed -n '3,/^set -eu/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'
}

die() {
	printf 'export-layout: %s\n' "$*" >&2
	exit 1
}

files=no
case ${1-} in
--help | -h)
	usage
	exit 0
	;;
--files)
	files=yes
	shift
	;;
esac
if [ $# -ne 3 ]; then
	usage >&2
	exit 2
fi
layout=$1 ref=$2 dir=$3

for tool in jq sha256sum zstd; do
	command -v "$tool" >/dev/null || die "needs $tool"
done
if [ "$files" = yes ]; then
	command -v mcopy >/dev/null || die "--files needs mtools (mcopy)"
fi
if [ -e "$dir" ] && [ -n "$(ls -A "$dir")" ]; then
	die "$dir is not empty"
fi
# Long names and the comparison of names ignoring case need a UTF-8
# locale; mtools reads no configuration file of the host.
utf8=
for candidate in C.UTF-8 C.utf8 en_US.UTF-8 en_US.utf8; do
	if locale -a 2>/dev/null | grep -qx "$candidate"; then
		utf8=$candidate
		break
	fi
done
[ -n "$utf8" ] || die "needs a UTF-8 locale (C.UTF-8 or en_US.UTF-8)"
LC_ALL=$utf8
MTOOLSRC=/dev/null
export LC_ALL MTOOLSRC

# The file of a blob, and a check of a file against a digest.
blob() { printf '%s/blobs/%s/%s' "$layout" "${1%%:*}" "${1#*:}"; }
verify() {
	[ "sha256:$(sha256sum <"$2" | cut -d' ' -f1)" = "$1" ] || die "$2 does not match $1"
}

case $ref in
sha256:*) manifest=$ref ;;
*)
	manifest=$(jq -r --arg ref "$ref" '[.manifests[]
		| select(.annotations["org.opencontainers.image.ref.name"] == $ref) | .digest]
		| if length == 1 then .[0] else empty end' "$layout/index.json")
	[ -n "$manifest" ] || die "$ref names no single image in $layout/index.json"
	;;
esac
verify "$manifest" "$(blob "$manifest")"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

tab=$(printf '\t')
# One layer a line, its fields separated by the unit separator 1Fh, which no
# field holds; a separator that is not white space keeps empty fields.
us=$(printf '\037')
jq -r '.layers[] | [.digest, .mediaType,
	(.annotations["org.opencontainers.image.title"] // ""),
	(.annotations["unemu.decoded.digest"] // ""), "end"] | join("\u001f")' \
	"$(blob "$manifest")" >"$tmp/layers"

# The names first, as unemu checks them (machine-images §2, Export file
# names): a title, and without .zst the decoded name of a compressed layer,
# is one file name; no two layers take names equal ignoring case; and no
# layer takes a name the export writes itself, but a set's metadata layer
# metadata.json. Nothing more is written until they pass.
printf '%s\n' config.toml manifest.json EXPORT-VERSION SHA256SUMS >"$tmp/taken"
while IFS=$us read -r digest type title decoded end; do
	[ "$end" = end ] || die "layer $digest: unreadable descriptor"
	case $type in
	*+zstd)
		name=${title%.zst}
		[ "$name" != "$title" ] || die "the compressed layer $title does not end in .zst"
		;;
	*) name=$title ;;
	esac
	for taken in "$title" "$name"; do
		case $taken in
		"" | . | .. | */*) die "layer $digest has no usable file name: '$title'" ;;
		esac
	done
	if [ "$type" != application/vnd.unemu.metadata.v1+json ] &&
		printf '%s\n' "$name" | grep -Fxiq metadata.json; then
		die "the layer $title takes metadata.json, a name the export writes itself"
	fi
	if grep -Fxiq -- "$name" "$tmp/taken" ||
		{ [ "$title" != "$name" ] && grep -Fxiq -- "$title" "$tmp/taken"; }; then
		die "the layer $title takes a name another layer or the export takes, ignoring case"
	fi
	printf '%s\n' "$name" >>"$tmp/taken"
	[ "$title" = "$name" ] || printf '%s\n' "$title" >>"$tmp/taken"
done <"$tmp/layers"
if [ "$files" = yes ] && grep -Fxiq files "$tmp/taken"; then
	die "--files writes the folder files, and a layer takes that name"
fi

mkdir -p "$dir"
cat "$(blob "$manifest")" >"$dir/manifest.json"
config=$(jq -r .config.digest "$dir/manifest.json")
verify "$config" "$(blob "$config")"
cat "$(blob "$config")" >"$dir/config.toml"
printf '1\n' >"$dir/EXPORT-VERSION"
printf '%s\n' config.toml manifest.json EXPORT-VERSION >"$tmp/names"
: >"$tmp/media"
while IFS=$us read -r digest type title decoded end; do
	verify "$digest" "$(blob "$digest")"
	case $type in
	*+zstd)
		name=${title%.zst}
		zstd -d -q -c "$(blob "$digest")" >"$dir/$name"
		verify "$decoded" "$dir/$name"
		;;
	*)
		name=$title
		cat "$(blob "$digest")" >"$dir/$name"
		;;
	esac
	printf '%s\n' "$name" >>"$tmp/names"
	case $type in
	application/vnd.unemu.media.floppy.v1 | application/vnd.unemu.media.floppy.v1+zstd)
		printf 'floppy%s%s\n' "$us" "$name" >>"$tmp/media"
		;;
	application/vnd.unemu.media.optical.v1 | application/vnd.unemu.media.optical.v1+zstd)
		printf 'optical%s%s\n' "$us" "$name" >>"$tmp/media"
		;;
	esac
done <"$tmp/layers"

# Merge the tree in $1 into $2: a folder both hold is one folder, a file
# both hold must have the same bytes. Folders made here are listed with the
# volume number $3, whose folder's time they take at the end.
merge() {
	(cd "$1" && find . | LC_ALL=C sort) | while IFS= read -r path; do
		[ "$path" != . ] || continue
		path=${path#./}
		if [ -d "$1/$path" ]; then
			if [ ! -e "$2/$path" ]; then
				mkdir "$2/$path"
				printf '%s\t%s\n' "$3" "$path" >>"$tmp/folders"
			elif [ ! -d "$2/$path" ]; then
				die "files/$path is a file on one diskette and a folder on another"
			fi
		elif [ -e "$2/$path" ]; then
			if [ ! -f "$2/$path" ] || ! cmp -s "$1/$path" "$2/$path"; then
				die "two diskettes hold files/$path with other bytes"
			fi
		else
			cp -p "$1/$path" "$2/$path"
		fi
	done
}

if [ "$files" = yes ]; then
	mkdir "$dir/files"
	: >"$tmp/folders"
	volume=0
	while IFS=$us read -r kind name; do
		if [ "$kind" = optical ]; then
			printf 'not read\t%s\n' "$name"
			continue
		fi
		volume=$((volume + 1))
		mkdir "$tmp/volume$volume"
		if ! MTOOLS_SKIP_CHECK=1 mdir -i "$dir/$name" :: >/dev/null 2>&1; then
			printf 'not read\t%s\n' "$name"
			continue
		fi
		if [ -n "$(MTOOLS_SKIP_CHECK=1 mdir -a -b -i "$dir/$name" :: 2>/dev/null)" ]; then
			MTOOLS_SKIP_CHECK=1 TZ=UTC0 mcopy -s -m -n -Q -i "$dir/$name" '::*' "$tmp/volume$volume/"
		fi
		merge "$tmp/volume$volume" "$dir/files" "$volume"
	done <"$tmp/media"
	# A folder's time is its entry's on the first diskette that holds it,
	# set after every file went in; innermost first.
	LC_ALL=C sort -t "$tab" -k 2 -r "$tmp/folders" | while IFS=$tab read -r volume path; do
		touch -m -r "$tmp/volume$volume/$path" "$dir/files/$path"
	done
fi

(cd "$dir" && LC_ALL=C sort "$tmp/names" | while IFS= read -r name; do
	sha256sum -- "$name"
done) >"$tmp/sums"
cat "$tmp/sums" >"$dir/SHA256SUMS"
