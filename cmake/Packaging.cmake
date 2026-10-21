# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Packaging of the binary `.deb`. It uses the same install tree that the
# per-component install tests check. Requires cmake/Backends.cmake, which
# defines the five install components and their directories.

set(CPACK_PACKAGE_NAME "${PACKAGE_NAME}")
set(CPACK_PACKAGE_VERSION "${VERSION}")
set(CPACK_PACKAGE_VENDOR "Tobias Mueller")
set(CPACK_PACKAGE_CONTACT "Tobias Mueller <muelli@cryptobitch.de>")
set(CPACK_PACKAGE_DESCRIPTION_SUMMARY "JMAP support for GNOME Evolution")
set(CPACK_PACKAGE_HOMEPAGE_URL "https://github.com/muelli/evolution-jmap")

# DEB is the base generator. cmake/PackagingRpm.cmake adds RPM only when
# rpmbuild is available. The other generators would make a .tar.gz of absolute
# paths under /usr, which nobody must unpack by accident.
set(CPACK_GENERATOR "DEB")

# One package, made from named components, not from all of the install tree.
# `src/` installs the upstream C example module into Evolution's module
# directory with no COMPONENT. A monolithic package would thus ship a
# demonstration module to each machine. The list of five components excludes
# it, and cmake/tests/check-deb-package.cmake fails if it comes back.
set(CPACK_DEB_COMPONENT_INSTALL ON)
set(CPACK_COMPONENTS_GROUPING ALL_COMPONENTS_IN_ONE)
set(CPACK_COMPONENTS_ALL
	book-backend
	cal-backend
	camel-provider
	collection-backend
	config-module
)

# The translation catalogues, if po/LINGUAS lists any. An empty list makes no
# install rule for the translations component, and CPack must not get a
# component that does not exist. The equality check below then makes sure that
# the package contains each catalogue and is not silently untranslated.
if(INSTALLED_CATALOGUES)
	list(APPEND CPACK_COMPONENTS_ALL translations)
endif()

# `<name>_<version>_<arch>.deb`: the name that tools which read a directory of
# packages expect. The CPack default is a CMake-style name.
set(CPACK_DEBIAN_FILE_NAME "DEB-DEFAULT")
set(CPACK_DEBIAN_PACKAGE_SECTION "gnome")
set(CPACK_DEBIAN_PACKAGE_PRIORITY "optional")
set(CPACK_DEBIAN_PACKAGE_HOMEPAGE "${CPACK_PACKAGE_HOMEPAGE_URL}")

# Calculate the dependencies, do not write them by hand. The address book and
# calendar factories, Camel, the registry and the Evolution shell dlopen these
# modules. A library that is missing at runtime thus causes no clear error: the
# backend is only absent from the account type list. Only dpkg-shlibdeps,
# which reads the package's ELF files, knows the true list. The list stays
# correct when the EDS version used for the build changes.
set(CPACK_DEBIAN_PACKAGE_SHLIBDEPS ON)

# Evolution keeps its own libraries in a private directory (`privlibdir`,
# /usr/lib/evolution), not on the default loader path. Each module that
# Evolution dlopens (ours also) links libevolution-mail.so.0 and
# libevolution-shell.so.0. The loader finds them at runtime because the shell
# process has already loaded them. dpkg-shlibdeps searches only the default
# paths. Without this setting it fails with `cannot find library
# libevolution-mail.so.0`, and CPack makes no package. Get the directory from
# pkg-config, as for all other directories here.
pkg_check_variable(EVOLUTION_PRIVATE_LIB_DIR evolution-shell-3.0 privlibdir)
if(NOT EVOLUTION_PRIVATE_LIB_DIR)
	message(FATAL_ERROR
		"evolution-shell-3.0 reports no privlibdir; dpkg-shlibdeps would fail "
		"to resolve the libraries the configuration module links")
endif()
set(CPACK_DEBIAN_PACKAGE_SHLIBDEPS_PRIVATE_DIRS "${EVOLUTION_PRIVATE_LIB_DIR}")

# lintian's no-changelog and no-copyright-file: Debian policy §12.3/§12.5
# requires both under /usr/share/doc/<package>/ in each package. lintian checks
# this also if the .deb never goes into a Debian archive.
#
# The changelog is a plain-text upstream file (docs/packaging/changelog).
# `gzip -n` keeps the original name and modification time out of the
# compressed data, so the bytes depend only on the changelog content.
# cmake/tests/check-deb-reproducible.cmake requires this.
#
# The copyright file (docs/packaging/copyright) is in DEP-5 format.
# tools/generate-debian-copyright.py (Track C2) makes it from REUSE.toml. The
# same script also makes the third-party-notices file. The
# debian-copyright-in-sync CTest keeps both files in sync.
find_program(GZIP_EXECUTABLE gzip REQUIRED)
set(_changelog_build "${CMAKE_BINARY_DIR}/changelog")
set(_changelog_gz "${CMAKE_BINARY_DIR}/changelog.gz")
configure_file(
	"${CMAKE_SOURCE_DIR}/docs/packaging/changelog"
	"${_changelog_build}"
	COPYONLY
)
add_custom_command(
	OUTPUT "${_changelog_gz}"
	COMMAND ${GZIP_EXECUTABLE} -n -9 -f -k "${_changelog_build}"
	DEPENDS "${_changelog_build}"
	COMMENT "Compressing the upstream changelog for packaging"
)
add_custom_target(packaging-changelog ALL DEPENDS "${_changelog_gz}")

# These files go in one component (config-module), not in all five.
# ALL_COMPONENTS_IN_ONE merges all components into the single .deb, so the
# selection of the component has no effect.
install(FILES "${_changelog_gz}"
	DESTINATION "/usr/share/doc/${PACKAGE_NAME}"
	COMPONENT config-module
)
install(FILES "${CMAKE_SOURCE_DIR}/docs/packaging/copyright"
	DESTINATION "/usr/share/doc/${PACKAGE_NAME}"
	COMPONENT config-module
)
# Track C2: the third-party Cargo crates that are statically linked into the
# shipped .so files. The DEP-5 stanzas of docs/packaging/copyright cannot list
# them correctly (refer to the comment in that file).
install(FILES "${CMAKE_SOURCE_DIR}/docs/packaging/third-party-notices"
	DESTINATION "/usr/share/doc/${PACKAGE_NAME}"
	COMPONENT config-module
)

# Track C1: the package has one RUNPATH entry (/usr/lib/evolution, which
# module-jmap-configuration.so needs). It is intentional: all Evolution modules
# have it. Thus a written justification satisfies lintian's
# custom-library-search-path, and the module keeps the RUNPATH it needs to load.
# Debian policy gives the override file the package name, with no extension.
install(FILES "${CMAKE_SOURCE_DIR}/docs/packaging/lintian-overrides"
	DESTINATION "/usr/share/lintian/overrides"
	RENAME "${PACKAGE_NAME}"
	COMPONENT config-module
)

# Only the *extended* description. CPack puts
# CPACK_PACKAGE_DESCRIPTION_SUMMARY on the synopsis line and indents each line
# below by one space, as Debian policy requires. Do not repeat the summary or
# indent these lines here. Either error gives a duplicate synopsis or a
# description that apt shows unwrapped. cmake/tests/check-deb-package.cmake
# rejects both.
set(CPACK_DEBIAN_PACKAGE_DESCRIPTION
	"Backends that let Evolution and evolution-data-server speak JMAP
(RFC 8620/8621): an address book backend, a calendar backend, a Camel mail
provider, the collection backend that fans one account out into all three,
and the account setup module.
.
The modules are dlopened by evolution-data-server and by Evolution itself,
and must be used with the versions they were built against.")

# Gives each entry in the package (files, the directories CPack makes for them,
# and the ar member headers) the timestamp of this build. Without it, the dates
# come from the packaging clock and the link times of the modules.
# CPack's DEB generator reads SOURCE_DATE_EPOCH from the environment of the
# `cpack` process, so this must be a file, not a variable. Set it before
# include(CPack), which writes the path into CPackConfig.cmake.
# cmake/tests/check-deb-reproducible.cmake makes the package three times in
# three different environments and fails if the results are different.
set(CPACK_PROJECT_CONFIG_FILE "${CMAKE_BINARY_DIR}/cpack-project-config.cmake")
configure_file(
	"${CMAKE_SOURCE_DIR}/cmake/cpack-project-config.cmake.in"
	"${CPACK_PROJECT_CONFIG_FILE}"
	@ONLY
)

include(cmake/PackagingRpm.cmake)

include(CPack)

# All regular files that the package must contain. The check is an equality,
# so the package must contain no other files. These are the same expressions
# that the install rules use, so a moved module moves here too. The install-*
# tests use pkg-config to check that the directories are correct.
set(EXPECTED_PACKAGE_FILES
	${EDS_BOOK_BACKEND_DIR}/libebookbackendjmap.so
	${EDS_CAL_BACKEND_DIR}/libecalbackendjmap.so
	${CAMEL_PROVIDER_DIR}/libcameljmap.so
	${CAMEL_PROVIDER_DIR}/libcameljmap.urls
	${EDS_REGISTRY_MODULE_DIR}/module-jmap-backend.so
	${EVOLUTION_MODULE_DIR}/module-jmap-configuration.so
	/usr/share/doc/${PACKAGE_NAME}/changelog.gz
	/usr/share/doc/${PACKAGE_NAME}/copyright
	/usr/share/doc/${PACKAGE_NAME}/third-party-notices
	/usr/share/lintian/overrides/${PACKAGE_NAME}
)

# add_translations() gives these paths, so a catalogue cannot go to a path
# that the package check does not know.
list(APPEND EXPECTED_PACKAGE_FILES ${INSTALLED_CATALOGUES})

string(REPLACE ";" "|" _expected_package_files "${EXPECTED_PACKAGE_FILES}")

add_test(
	NAME package-deb
	COMMAND ${CMAKE_COMMAND}
		"-DBUILD_DIR=${CMAKE_BINARY_DIR}"
		"-DSTAGE_DIR=${CMAKE_BINARY_DIR}/package-test"
		"-DPACKAGE_NAME=${PACKAGE_NAME}"
		"-DPACKAGE_VERSION=${VERSION}"
		"-DPACKAGE_SUMMARY=${CPACK_PACKAGE_DESCRIPTION_SUMMARY}"
		"-DEXPECTED=${_expected_package_files}"
		-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-deb-package.cmake"
)

# package-deb runs cpack with `-G DEB`. The release workflow builds the
# `package` target (`cmake --build build --target package`, no `-G`), which
# runs all generators in CPACK_GENERATOR. A generator without its toolchain
# (RPM without rpmbuild, on the release image) can then stop the full run,
# while all `-G DEB` tests pass.
add_test(
	NAME package-default-generator
	COMMAND ${CMAKE_COMMAND}
		"-DBUILD_DIR=${CMAKE_BINARY_DIR}"
		-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-package-default-generator.cmake"
)

# The two epochs are intentionally incorrect and different. The test exports
# each one and requires that the package ignores both. They are 2020-09-13 and
# 2023-11-14, before all content of this tree, so a package that used them is
# easy to see in the failure output.
add_test(
	NAME package-deb-reproducible
	COMMAND ${CMAKE_COMMAND}
		"-DBUILD_DIR=${CMAKE_BINARY_DIR}"
		"-DSTAGE_DIR=${CMAKE_BINARY_DIR}/package-reproducible-test"
		"-DEPOCH_A=1600000000"
		"-DEPOCH_B=1700000000"
		-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-deb-reproducible.cmake"
)

# The package tests above are useful only if a release contains the package.
# Only a tag push runs the release workflow, not this build or CI. Thus this
# test checks the workflow as a document: it publishes what it attests, the
# package is included, and it builds in the same pinned image as CI.
add_test(
	NAME release-workflow
	COMMAND ${CMAKE_COMMAND}
		"-DSOURCE_DIR=${CMAKE_SOURCE_DIR}"
		-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-release-workflow.cmake"
)

# Track C1: CI keeps the .deb lintian-clean. lintian is optional: not all build
# machines have it (`ci/install-deps.sh` installs it for CI). Thus the test is
# registered only when `find_program` finds lintian, while the package tests
# above require cpack/dpkg.
find_program(LINTIAN_EXECUTABLE lintian)
if(LINTIAN_EXECUTABLE)
	add_test(
		NAME package-deb-lintian
		COMMAND ${CMAKE_COMMAND}
			"-DBUILD_DIR=${CMAKE_BINARY_DIR}"
			"-DSTAGE_DIR=${CMAKE_BINARY_DIR}/package-lintian-test"
			"-DLINTIAN_EXECUTABLE=${LINTIAN_EXECUTABLE}"
			-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-deb-lintian.cmake"
	)
endif()

# Track C2: makes sure that docs/packaging/copyright (made by
# tools/generate-debian-copyright.py) stays in sync with REUSE.toml, and does
# not become a hand-edited file again.
find_program(PYTHON3_EXECUTABLE python3)
if(PYTHON3_EXECUTABLE)
	add_test(
		NAME debian-copyright-in-sync
		COMMAND ${CMAKE_COMMAND}
			"-DSOURCE_DIR=${CMAKE_SOURCE_DIR}"
			"-DPYTHON3_EXECUTABLE=${PYTHON3_EXECUTABLE}"
			-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-debian-copyright.cmake"
	)
endif()

# The system-library part of the release SBOM. ci/deb-dependencies.sh resolves
# the Depends of the .deb to installed versions on the build host. ci/sbom.sh
# adds them on the release runner. Both run only on a tag, so these tests run
# them on temporary packages with Depends that the test selects. Debian-family
# hosts only. The second test also needs the pinned SBOM generator, which only
# the release workflow installs.
find_program(DPKG_DEB_EXECUTABLE dpkg-deb)
find_program(JQ_EXECUTABLE jq)
if(DPKG_DEB_EXECUTABLE AND JQ_EXECUTABLE)
	add_test(
		NAME sbom-deb-dependencies
		COMMAND ${CMAKE_COMMAND}
			"-DSOURCE_DIR=${CMAKE_SOURCE_DIR}"
			"-DSTAGE_DIR=${CMAKE_BINARY_DIR}/sbom-deb-dependencies-test"
			-DPART=dependencies
			-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-sbom.cmake"
	)
	find_program(CARGO_CYCLONEDX_EXECUTABLE cargo-cyclonedx)
	if(CARGO_CYCLONEDX_EXECUTABLE)
		add_test(
			NAME sbom-reproducible
			COMMAND ${CMAKE_COMMAND}
				"-DSOURCE_DIR=${CMAKE_SOURCE_DIR}"
				"-DSTAGE_DIR=${CMAKE_BINARY_DIR}/sbom-reproducible-test"
				-DPART=sbom
				-P "${CMAKE_SOURCE_DIR}/cmake/tests/check-sbom.cmake"
		)
	endif()
endif()
