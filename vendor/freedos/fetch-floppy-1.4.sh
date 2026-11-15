#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Fetch the official FreeDOS 1.4 Floppy Edition and the corresponding source
# of every program on its disks, for the library's installation media sets
# (see README.md, "FreeDOS 1.4 Floppy Edition"). Nothing is committed and
# nothing downloaded is ever executed: every file is checked against its
# pinned SHA-256, the disk images are copied out of the edition unmodified,
# and the sources are packed into one deterministic tar. When every output
# already matches its pin the script stops there, without the network.
#
# Requires curl, unzip, git, python3 and GNU tar on PATH. Run it inside
# `nix develop`: the flake devShell provides all of them and the pinned CA
# roots that the downloads are verified against.
set -euo pipefail

# BSD tar (the macOS default) cannot fix member order, owners and modes, and
# GNU tar would take extra options from TAR_OPTIONS.
unset TAR_OPTIONS
case "$(tar --version 2>/dev/null)" in
	*"GNU tar"*) ;;
	*)
		echo "missing GNU tar: run this script inside nix develop" >&2
		exit 1
		;;
esac

# Both GNU/Linux and macOS provide one of these SHA-256 tools.
if command -v sha256sum >/dev/null 2>&1; then
	SHA256=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
	SHA256=(shasum -a 256)
else
	echo "missing SHA-256 tool: install sha256sum or shasum (or use nix develop)" >&2
	exit 1
fi

sha256_of() {
	"${SHA256[@]}" "$1" | awk '{print $1}'
}

# Whether a file exists and holds the pinned bytes.
matches() {
	[ -f "$1" ] && [ "$(sha256_of "$1")" = "$2" ]
}

check_sha256() {
	local file="$1" expected="$2" actual
	actual="$(sha256_of "$file")"
	if [ "$actual" != "$expected" ]; then
		echo "SHA-256 mismatch for $file: expected $expected, got $actual" >&2
		if [ "${3:-output}" = download ]; then
			echo "Remove the cached file and fetch the pinned release again; do not change its expected hash." >&2
		else
			echo "Check the pinned inputs, the licence index and the GNU tar version before updating the expected hash." >&2
		fi
		exit 1
	fi
}

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$HERE/build/floppy-1.4"   # gitignored downloads and clones
OUT="$HERE/floppy-1.4"          # gitignored disks, licence index and source tar

# Pins of every output: the disks unmodified, by format directory and
# installer order, the licence index and the source archive.
IMAGES="120m/x86BOOT.img 03df6088be016e57a6c44275f5bb9ab0244db71de1360957fd76ba83243b6a77
120m/x86DSK01.img 83505b32f4165f393e76d5de3e59610bf49ffad6c349fd68a2f4795e0d5153af
120m/x86DSK02.img 05303837dbd336c3fdc20fe55ea3e2778de467ecf2a68dbce905c721b0ddb94c
120m/x86DSK03.img f82ebe5153a2fb5a37a1569c8927447c086b2d06c77c979bea4ad65b14b4b111
120m/x86DSK04.img ddbbeb52f90c86ef4952a6c4e8b923577071712427a9cbb1578964fa8c9c7487
120m/x86DSK05.img e1687f7672e707cc7e70ac4b11a4a0bb6a4e37a05f1d93914d6867d481338768
120m/x86DSK06.img 57b2e375740c061438f8e1446afd202a718278f3c20713c444454cf379afc913
120m/x86DSK07.img b5b7cc6dc0f85ad9c7a961311c3ccf2d811ae0dd37104f44bee8c31a2bc06fe3
144m/x86BOOT.img 552f7cbb0625960c050a3e682a4d2121cc2ffdfa9dc9d592ccd49edf179333dc
144m/x86DSK01.img b9dd5315f50a8b770b3d2ecb623b18ed067660e22f421504d199254f7f653516
144m/x86DSK02.img 4a951f15da8dd98e54ea84f07fe2a7802e914aa8e66942f646d2e11ca3943e59
144m/x86DSK03.img fbfd172e1ac4bc4abb3f656cae94e2727d2b48b4d02cef0362564048f83f35da
144m/x86DSK04.img 894d49c4dfdd294e33878ca317e19a0322f267f639b247a6b2a669799b4720f8
144m/x86DSK05.img 0362e084497c5c995ccab1523c4e39f94b7139f988ffa3a4f7e871452c352f3c
144m/x86DSK06.img 7a93d1d4e47f5edd5d0a9f17d57c3417ae135052db9449a67aa374a3b4316d80
720k/x86BOOT.img cbca90627b944319f02043bc41f1715ff5366b23e5e8e75aa3ca23bb564e0789
720k/x86DSK01.img 02650e6d952aec1b7465574cc8ddb0f29058444029050f06ab29094589b2b889
720k/x86DSK02.img ad2c055b22312f217a76bf0af25be6bd09b5e208b416d4d51cd7cc8bf9d0cd57
720k/x86DSK03.img 49d79c2a4d36bb026008f31ebf8e7597330a04a0f396111862e6d36ba2398613
720k/x86DSK04.img 17f756003d3436b5371ab3108c37a73e78eb028ed89a75b067abde3f5ceb7e62
720k/x86DSK05.img c3aeb780e1a2cabc5c50e30d2078741a3026da9085ef052d4edaa42ee2b9685c
720k/x86DSK06.img d54ce27af491a15128d61f6ee9581cec92c4e823e59e79bb8875147494a0123f
720k/x86DSK07.img ea342ba1cb40ba253540d4a935b2ae2834d96d0a35e943f32caaa38684601425
720k/x86DSK08.img 69ee59879182cdad0bf1a03b318a30dd69c11cd9c4985d875e4bf1587c922fec
720k/x86DSK09.img f344088ceb48f05c718d0158a902e584c1f66a7e0d8d67ab2adf9069e122051e
720k/x86DSK10.img de31af59860e1f59a67a2036113a0af4db8f72bebb4812fe5aa291c5faf3cce0
720k/x86DSK11.img 70e82a287d4d89b40c975d6a62700633cdd6dac0d55da53953edfc37c69a6c0b"
LICENSES_SHA256="35d9478a2f5b3e54ded8d20b1b23feec10b0abd2e1e6715c77ddf314e7bf1a7f"
SOURCE_NAME="freedos-1.4-floppy-src"
SOURCE_ARCHIVE_SHA256="dfa7006e742cb9676a0a6401ce64214089923f055f4a7ba8f36960115b5362b6"

# Fast path: outputs that all match their pins need nothing else, so a cached
# copy is re-checked byte for byte and never trusted unverified.
outputs_match() {
	local member expected
	while read -r member expected; do
		matches "$OUT/$member" "$expected" || return 1
	done < <(echo "$IMAGES")
	matches "$OUT/LICENSES.TXT" "$LICENSES_SHA256" \
		&& matches "$OUT/$SOURCE_NAME.tar" "$SOURCE_ARCHIVE_SHA256"
}
if outputs_match; then
	echo "every output in $OUT matches its pin"
	exit 0
fi

mkdir -p "$WORK" "$OUT"
STAGE_ROOT=""
# The extracted ISO (about 300 MB), partial downloads and the source staging
# directory never outlive the run, whatever stops it.
cleanup() {
	rm -f "${WORK:?}/FD14LIVE.iso" "${WORK:?}"/*.part
	if [ -n "$STAGE_ROOT" ]; then
		rm -r -f "${STAGE_ROOT:?}"
	fi
}
trap cleanup EXIT

# Fetch one pinned file into the cache: each URL in turn, until a download
# matches the pin. A bad response never reaches the cache name.
download() {
	local dest="$1" expected="$2" url
	shift 2
	if [ -f "$dest" ]; then
		check_sha256 "$dest" "$expected" download
		return
	fi
	for url in "$@"; do
		rm -f "$dest.part"
		if curl -fsSL -o "$dest.part" "$url" && matches "$dest.part" "$expected"; then
			mv "$dest.part" "$dest"
			return
		fi
		echo "$url failed or did not match the pinned SHA-256" >&2
	done
	rm -f "$dest.part"
	echo "no source of $(basename "$dest") gave the pinned bytes" >&2
	exit 1
}

# The release directory and its mirror on ibiblio, where FreeDOS keeps every
# distribution. Both serve the same bytes; the hashes decide.
PRIMARY="https://download.freedos.org/1.4"
MIRROR="https://www.ibiblio.org/pub/micro/pc-stuff/freedos/files/distributions/1.4"
release() {
	# $1 file name in the release directory, $2 sha256, $3 destination
	download "$3" "$2" "$PRIMARY/$1" "$MIRROR/$1"
}

# --- the edition ------------------------------------------------------------
# FD14-FloppyEdition.zip and FD14-LiveCD.zip carry the hashes the release's
# own verify.txt lists.
EDITION_SHA256="45b1fa7c52dd996c3bfa5e352ffcd410781b952a6ad629f15a4c9ec4bbaefc5a"
release FD14-FloppyEdition.zip "$EDITION_SHA256" "$WORK/FD14-FloppyEdition.zip"

# The edition's complete member list: three directories of disk images, one
# per floppy format, and readme.txt. Anything else refuses the archive.
EXPECTED_MEMBERS="120m/
120m/x86BOOT.img
120m/x86DSK01.img
120m/x86DSK02.img
120m/x86DSK03.img
120m/x86DSK04.img
120m/x86DSK05.img
120m/x86DSK06.img
120m/x86DSK07.img
144m/
144m/x86BOOT.img
144m/x86DSK01.img
144m/x86DSK02.img
144m/x86DSK03.img
144m/x86DSK04.img
144m/x86DSK05.img
144m/x86DSK06.img
720k/
720k/x86BOOT.img
720k/x86DSK01.img
720k/x86DSK02.img
720k/x86DSK03.img
720k/x86DSK04.img
720k/x86DSK05.img
720k/x86DSK06.img
720k/x86DSK07.img
720k/x86DSK08.img
720k/x86DSK09.img
720k/x86DSK10.img
720k/x86DSK11.img
readme.txt"
unzip -l "$WORK/FD14-FloppyEdition.zip"
if [ "$(unzip -Z1 "$WORK/FD14-FloppyEdition.zip")" != "$EXPECTED_MEMBERS" ]; then
	echo "FD14-FloppyEdition.zip does not hold the expected members" >&2
	exit 1
fi
echo "$IMAGES" | while read -r member expected; do
	mkdir -p "$OUT/$(dirname "$member")"
	unzip -p "$WORK/FD14-FloppyEdition.zip" "$member" > "$OUT/$member.part"
	check_sha256 "$OUT/$member.part" "$expected"
	mv "$OUT/$member.part" "$OUT/$member"
done

# --- the corresponding source ------------------------------------------------
# The release's packages with their SOURCE/ trees, exactly as the LiveCD
# carries them (the online repository has moved on since the release). DHCP
# in fdnet is mTCP's, so mTCP's package comes too; fdinst is part of fdnpkg.
LIVECD_SHA256="2020ff6bb681967fd6eff8f51ad2e5cd5ab4421165948cef4246e4f7fcaf6339"
LIVECD_ISO_SHA256="c48a9dcf4b8e22f44e268a9879745f0bd88c061195ac584e6ef2deb0477f81fb"
PACKAGES="apps/fdimples 3259e011cad448a20863c1ebdab2fb1d509b829765efe0d1ad5bc2cac3857fcf
archiver/gzip a21e7ffa045ee5c19be7e49757bfe03828295a1a3ab67bf54f2bf7a2182de8c5
archiver/slicer 48598beb09e72548710b68e4699b51c16694a90801764328f12b5cb1cb2962a2
archiver/unzip ce22cde8ee2185f7e9312a61413ce2fe7879df9aebbf1cf04f0c4181456e4c17
archiver/zip e139f09786aa77678d06c6a266e324bd74bd0a82ee4ebd95da85c5c2576dc4dc
base/append 5bbd29b12309cc6966796f90a2580714ea902eb19e744a7d2905bb5b2a9144c6
base/assign 6e79e0fc10d18d5153f7c42b5cbbbfe0c64575c0b79f398dd66b6383b07beb49
base/attrib b5a77bd0c20a821bf0085dfed7cff408a8dd72603506c0abda1e2c54c6303964
base/chkdsk 42a7120307e6604147ee663b3cba2b346bdb1305961eed0357ec6130cd23ba4e
base/choice fae8902974b165dbaaa47c87ad171d0a073f94f0610a71ebec0c5f808a68079b
base/comp 3687e25eb78f1164a66e4f7328bc37952c402fba3ba888df64ad59f323114d96
base/cpidos 3a3daed5de2daf7ed39d0f3f8bb8b96ff23a79e2cda5622a1d8796d197261e7c
base/ctmouse dd378ddfe5b6948d3073c12acaf0ca31bf60d0898f1cd708b6858083a14a41c7
base/debug 0ceabfacdad7066d159419bedd2a5299d583fb04ba0540d8a89df3e5563a09f7
base/defrag a2af8b47652616da9a33aee6e61193fdff729616120f594067a4c6faf95763de
base/deltree 6f603150a3d9004314dcfaba6c14f6c80e21dbfd2ef88aef602162bb2a4def3d
base/devload 8ed0fcd7ac0d66d9f6b351c5b5cf4c69026f60033bcb8b2552c3ae23c95c99d9
base/diskcomp e4f266f1e4e1d4dd8b90530ed9f911fcddb131d6d6008a4b524402561a56ac78
base/diskcopy 23eb5cbce5aa4ad767516d2ce259b76cfe3c82818783d5f7bb84745847fbc558
base/display f05a0130309c2e52620aa14f04e9c499c4f8c04a1e58266e9fa9baa873ee024e
base/edit d794ffb6c2a2d04f96a6354ac0eded9c05f5592e7e856c3c28b48e4ba1cb030e
base/edlin f8729654f985160867d0af99790515366b9067af6c83ff64e0d830533da92b5a
base/exe2bin a2e3cb2b29abf32a92b5a7fbe65f508b8f30df41141ac9eeb75c19b1bcc003b8
base/fc 5e1811bea9d60ffb34cef8e5851bc907cd57b130a37812480f5e759bcd94147d
base/fdapm bdda9abb631447508a659f1aec970079fe72b60a01c4e194255ffb34bfe77288
base/fdhelper 5180fa61c70ccf9084504100728d79e1aeb6d0103d4787b9e8f1cedb5e1e12ec
base/fdisk 1f705c34586e3468769e0baf59342891ac4c2329bbe87a140fcb4902381ab10d
base/fdxms 5fc8a472944211cfd6b456bc9753a061d6c969aaa16eda9541f983a19594d500
base/fdxms286 5586614769c542f75254a1caa562643089a17f6501e2507bfa8f488918fa8779
base/find 71115327e2a05fe723f5a22ef2505067f9176ddcc714a1e2856b8fafb97afe1b
base/format 80a7f0616fbe07ceb74c074bab2e79da880e6dd24cc588a43463a7182a312eae
base/freecom 2529cf15c2ee7d7030ed99a6a88df2cf5eef87b9fe10f1c0fb643c38ea6aaa8e
base/graphics c9a305e20cf78d684d4e6d30caa5cbe290296edc92917257119aa58be5b9209c
base/himemx 5be13efe2c86e784c546c2a79d7f1811d512d90d99a779c9ea204b4f4829615b
base/htmlhelp e420ca8223bfaf8ae2e64a8f17d582b0ee3f9c32b361bb8b25065b7a95735a3d
base/jemm c571724c2025cc493ba3b194ec68d29a2a9913a5e9b844828d3cdb4c21548994
base/kernel 38ce3c63e399c8f18ab6230d8988a5d1a1aa9be4e109d15c6f4842b5e8fe61e6
base/keyb 9fd172421cfd6f30bf45cd9e46e45c78e40439209104c4e81498e327904e759e
base/keyb_lay c20e750bd97eb85a1210500ab7f756f38a2070359eb53791a6a5979070f30bf6
base/label 40713091ecc0de5d3f053876e6b6e61ba5cd22e4276ad1dd7581108d5d136d90
base/lbacache b688c237ed99f71b0f23ded2abe2f63278456461e5bc9c6e3e42e465481b7a17
base/mem 05608dc39dad9e89f4b783aae710b2ebeedbf0ffa5453d45d9484445fd752799
base/mirror 728f2fb4410c3029ecfe374817bc1961589b53ddafd5aeb53259bc455dd22421
base/mkeyb 872b427c818b9fe286c0019aa49c3733c53aabeccb30f88cdc8e3f1eae6d139b
base/mode b4df1f5616afab1a7bf8774a1331914677fa7bd2ac3fd8d9dcde461c1601c492
base/more 6c2861a0a4fdbbca726e677cb45cc7761d4ba7f6926c3e7c6903e03723ef5869
base/move 0aeca7a82f1ea367e779ff736526316bf11c5cc7374e94c5de49bd72983e25d4
base/nansi be9e1179efe1ce5c4473d0a6f7d917566fdb7f8d60b63a846521ce32d14b2cf6
base/nlsfunc e8abc552205408422008d3c9b775e87e945f09f2717779d0a62d84ede59061aa
base/print ab10f50918c6936517044d0afd3cee0ba1a2676c0ffe0c6658a25a7f32c046db
base/recover 9c980817629463d865cee65f0fb22a9ac575da4049ec224ed026d428c4e09578
base/replace 3eb27db2fa1da396502f367aa4a35bcb54b6d5dabb341b5895fd20a4db365f5e
base/share be39883e7dda38f9cafcb9fc2bf37647d8adea1605838417bbea3e9efd979fc7
base/shsucdx 411eacf4f4b64cace0031462004f6018352de5de3509698734a2cb0f967de05d
base/sort 3f99a877e86a37a627bfa95906e9774447b0eb43a7174992723f37c0eea40aaf
base/swsubst aaa196da6a88ee8e2aa4de0ed7696d4ebc6f209eb5fcc9b9b8f035f8c26f9192
base/tree bb56442d17ec06b75ad15d99ca77dad879f9ad06539f3a3d9653902d8ba09da5
base/undelete 9e02ad4b204c832d4a142036601757ae741689c3de5a919c2070fccf1c43b80f
base/unformat 46fefe1b3c37c1b4ce84e300be06cc7e25035c1578ea5c5fc836b7e1c04c5f22
base/xcopy aa8ab3742dd0dd1b31ff991b3ed9bdf65f4f11f0da2c6a22b321c3d1e58f995b
drivers/shsufdrv adb35e7f33683f787b8f1da32e964c75195dbf74d088c5c2197d1a7738233a94
drivers/udvd2 c90d6611bb32e3dd3b7362db2b0a680665c6efcb9d0490dd2bbb8cb187fbe3a7
net/fdnet fa4d04aab2977c23845d5db779af068a2d2dde6789e0b4fdf34cd042ec8b5d2f
net/fdnpkg 38aeb7f2154cf4a159d7249e1704316922efedb112479dcab39fb85594a548a1
net/mtcp 79439cba08ae1e44339c0c97a9247494dbe6173d3c5b3397dc68b6354dbf4827
tools/callver 327d2a0ade12911cbdf4a42e9d700789e6847dfbe9732b5b57ba2ea6a5d86ea3
tools/cwsdpmi 4c0d6aee9b14609f2bb9b620d2245ec50c5c866a018860beb7e99d03ccad2219
tools/pkgtools 1ae990094f184fa1726642d6d578e2e68423ed896e75bb68e452563b5a89b335
tools/v8power f1378905558e07c9379bd0461043d869880440cc7a20429ce4472e12d36ac7e0"
# Package paths inside the LiveCD; the names hold no spaces. (Here-strings are
# avoided: some bash builds deadlock on a here-string larger than a pipe.)
# shellcheck disable=SC2207
MEMBERS=($(echo "$PACKAGES" | awk '{ print "packages/" $1 ".zip" }'))
missing=0
for member in "${MEMBERS[@]}"; do
	[ -f "$WORK/$member" ] || missing=1
done
# The LiveCD (about 300 MB) is needed only until every package is cached:
# its ISO is read by a small ISO 9660 reader, and both go once the packages
# verify.
if [ "$missing" -ne 0 ]; then
	release FD14-LiveCD.zip "$LIVECD_SHA256" "$WORK/FD14-LiveCD.zip"
	unzip -p "$WORK/FD14-LiveCD.zip" FD14LIVE.iso > "$WORK/FD14LIVE.iso"
	check_sha256 "$WORK/FD14LIVE.iso" "$LIVECD_ISO_SHA256"
	python3 -I "$HERE/iso-extract.py" "$WORK/FD14LIVE.iso" "$WORK" "${MEMBERS[@]}"
	rm -f "$WORK/FD14LIVE.iso"
fi
echo "$PACKAGES" | while read -r package expected; do
	check_sha256 "$WORK/packages/$package.zip" "$expected" download
done
rm -f "$WORK/FD14-LiveCD.zip"

# FDBANNER, on the boot disk only, is not on the LiveCD; the repository's
# package holds the same FDBANNER.COM and its source. It has no other source.
FDBANNER_SHA256="51d4ae13a3e1bb06eb05cc8b3e9ce8406349eebe806fd68fd9397cf10e2591c2"
mkdir -p "$WORK/packages/util"
download "$WORK/packages/util/fdbanner.zip" "$FDBANNER_SHA256" \
	"https://www.ibiblio.org/pub/micro/pc-stuff/freedos/files/repositories/1.4/util/fdbanner.zip"

# The installer, its translations and the release build environment, at the
# FreeDOS-v1.4 tags their projects set for this release.
clone_commit() {
	# $1 clone directory, $2 repository, $3 commit
	local dir="$1" repo="$2" commit="$3"
	if [ ! -d "$dir/.git" ]; then
		git init --quiet "$dir"
		git -C "$dir" remote add origin "$repo"
	fi
	if ! git -C "$dir" cat-file -e "$commit^{commit}" 2>/dev/null; then
		git -C "$dir" fetch --quiet --depth 1 origin "$commit"
	fi
}
clone_commit "$WORK/FDI-x86" "https://github.com/shidel/FDI-x86" "2ba66516bce2703a1b33e2c2190c7cebf339089f"
clone_commit "$WORK/FDI" "https://github.com/shidel/FDI" "23979ab50c9418052a84155795aa378b88bb73ab"
clone_commit "$WORK/fd-nls" "https://github.com/shidel/fd-nls" "079e0a38eef71d39559768f263ecd025257409da"
clone_commit "$WORK/builder" "https://gitlab.com/FreeDOS/OS/builder.git" "08f1920798d01276aca2576c74db7c1dcfd2870e"

# The licence index is the set's terms text; DOS readers expect CRLF.
awk '{ printf "%s\r\n", $0 }' "$HERE/floppy-1.4-licenses.txt" > "$OUT/LICENSES.TXT.part"
check_sha256 "$OUT/LICENSES.TXT.part" "$LICENSES_SHA256"
mv "$OUT/LICENSES.TXT.part" "$OUT/LICENSES.TXT"

STAGE_ROOT="$(mktemp -d "$WORK/source.XXXXXX")"
STAGE="$STAGE_ROOT/$SOURCE_NAME"
mkdir -p "$STAGE/installer/FDI-x86" "$STAGE/installer/FDI" "$STAGE/installer/fd-nls" "$STAGE/builder"
cp "$OUT/LICENSES.TXT" "$STAGE/LICENSES.TXT"
# Exactly the pinned packages, never whatever else the cache holds.
for package in $(echo "$PACKAGES" | awk '{ print $1 }') util/fdbanner; do
	mkdir -p "$STAGE/packages/$(dirname "$package")"
	cp "$WORK/packages/$package.zip" "$STAGE/packages/$package.zip"
done
# Line-ending and attribute settings are fixed so a host's git configuration
# cannot change the exported bytes.
export_tree() {
	# $1 clone, $2 commit, $3 destination, then optional paths
	local dir="$1" commit="$2" dest="$3"
	shift 3
	git -C "$dir" -c core.autocrlf=false -c core.eol=lf -c core.attributesFile=/dev/null \
		archive "$commit" -- "$@" | tar -x -C "$dest"
}
export_tree "$WORK/FDI-x86" 2ba66516bce2703a1b33e2c2190c7cebf339089f "$STAGE/installer/FDI-x86" .
export_tree "$WORK/FDI" 23979ab50c9418052a84155795aa378b88bb73ab "$STAGE/installer/FDI" .
# Only the translations and fonts the Floppy Edition installer uses; the rest
# of fd-nls translates programs whose packages carry their own NLS files.
export_tree "$WORK/fd-nls" 079e0a38eef71d39559768f263ecd025257409da "$STAGE/installer/fd-nls" \
	LICENSE README.md fdi/language fdi-x86
export_tree "$WORK/builder" 08f1920798d01276aca2576c74db7c1dcfd2870e "$STAGE/builder" .
# Member order, timestamps (the release date, 2025-04-02 00:00 UTC), owners
# and modes are fixed so the archive has the same bytes on every host; ustar
# stores no atime or ctime.
TEMPORARY_ARCHIVE="$STAGE_ROOT/source.tar"
LC_ALL=C tar --create --file="$TEMPORARY_ARCHIVE" --directory="$STAGE_ROOT" \
	--format=ustar --sort=name --hard-dereference --mtime="@1743552000" \
	--owner=0 --group=0 --numeric-owner --mode='a=rX,u+w,ug-s' "$SOURCE_NAME"
check_sha256 "$TEMPORARY_ARCHIVE" "$SOURCE_ARCHIVE_SHA256"
mv "$TEMPORARY_ARCHIVE" "$OUT/$SOURCE_NAME.tar"

outputs_match
echo "wrote $OUT: the 1.44M, 1.2M and 720K disks, LICENSES.TXT and $SOURCE_NAME.tar"
