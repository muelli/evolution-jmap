# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# M8: the binary `.deb`, built out of the same install tree the per-component
# install tests check. Requires cmake/Backends.cmake, which is where the five
# install components and the directories they land in are defined.

set(CPACK_PACKAGE_NAME "${PACKAGE_NAME}")
set(CPACK_PACKAGE_VERSION "${VERSION}")
set(CPACK_PACKAGE_VENDOR "Tobias Mueller")
set(CPACK_PACKAGE_CONTACT "Tobias Mueller <muelli@cryptobitch.de>")
set(CPACK_PACKAGE_DESCRIPTION_SUMMARY "JMAP support for GNOME Evolution")
set(CPACK_PACKAGE_HOMEPAGE_URL "https://github.com/muelli/evolution-jmap")

# Only the DEB generator is configured, and it is the only one `cpack` with no
# arguments should produce: the other generators would happily emit a .tar.gz
# of absolute paths under /usr, which is not something anyone should be able to
# unpack by accident.
set(CPACK_GENERATOR "DEB")

# One package, built from named components rather than from everything the
# install tree contains. The distinction is the whole reason this is not four
# lines: `src/` installs the upstream C example module into Evolution's module
# directory with no COMPONENT of its own, so a monolithic package would ship a
# demonstration module to every machine that installs JMAP support. Listing the
# five components is what excludes it, and cmake/tests/check-deb-package.cmake
# fails if it ever comes back.
set(CPACK_DEB_COMPONENT_INSTALL ON)
set(CPACK_COMPONENTS_GROUPING ALL_COMPONENTS_IN_ONE)
set(CPACK_COMPONENTS_ALL
	book-backend
	cal-backend
	camel-provider
	collection-backend
	config-module
)

# `<name>_<version>_<arch>.deb`, which is what every tool that reads a
# directory of packages expects; CPack's own default is a CMake-ish name.
set(CPACK_DEBIAN_FILE_NAME "DEB-DEFAULT")
set(CPACK_DEBIAN_PACKAGE_SECTION "gnome")
set(CPACK_DEBIAN_PACKAGE_PRIORITY "optional")
set(CPACK_DEBIAN_PACKAGE_HOMEPAGE "${CPACK_PACKAGE_HOMEPAGE_URL}")

# Derived, never written down. These modules are dlopened — by the address book
# and calendar factories, by Camel, by the registry and by Evolution's shell —
# so a library that is missing at runtime is not a loud failure but a backend
# that never appears in the account type list. dpkg-shlibdeps reading our own
# ELF files is the only thing that knows the true list, and it stays right when
# the EDS the package was built against changes underneath us.
set(CPACK_DEBIAN_PACKAGE_SHLIBDEPS ON)

# Evolution keeps its own libraries in a private directory of its own
# (`privlibdir`, /usr/lib/evolution) rather than on the loader's default path:
# libevolution-mail.so.0 and libevolution-shell.so.0 are linked by every module
# Evolution dlopens, ours included, and are found at runtime because the shell
# process has already loaded them. dpkg-shlibdeps does not run inside that
# process and searches only the default paths, so without this it does not
# merely miss a dependency — it fails outright, `cannot find library
# libevolution-mail.so.0`, and no package is produced at all. Asked of
# pkg-config rather than written down, like every other directory here.
pkg_check_variable(EVOLUTION_PRIVATE_LIB_DIR evolution-shell-3.0 privlibdir)
if(NOT EVOLUTION_PRIVATE_LIB_DIR)
	message(FATAL_ERROR
		"evolution-shell-3.0 reports no privlibdir; dpkg-shlibdeps would fail "
		"to resolve the libraries the configuration module links")
endif()
set(CPACK_DEBIAN_PACKAGE_SHLIBDEPS_PRIVATE_DIRS "${EVOLUTION_PRIVATE_LIB_DIR}")

# The *extended* description only: CPack puts
# CPACK_PACKAGE_DESCRIPTION_SUMMARY on the synopsis line and indents each line
# below by the one space Debian policy asks for. Repeating the summary here, or
# indenting these lines ourselves, is what produces a duplicated synopsis and a
# description apt renders unwrapped — both of which
# cmake/tests/check-deb-package.cmake refuses.
set(CPACK_DEBIAN_PACKAGE_DESCRIPTION
	"Backends that let Evolution and evolution-data-server speak JMAP
(RFC 8620/8621): an address book backend, a calendar backend, a Camel mail
provider, the collection backend that fans one account out into all three,
and the account setup module.
.
The modules are dlopened by evolution-data-server and by Evolution itself,
and must be used with the versions they were built against.")

include(CPack)

# Every regular file the package must contain — and, because the check is an
# equality, the only ones it may. Same expressions the install rules use, so a
# module that moves moves here too; that the directories themselves are right
# is the separate question the install-* tests ask of pkg-config.
set(EXPECTED_PACKAGE_FILES
	${EDS_BOOK_BACKEND_DIR}/libebookbackendjmap.so
	${EDS_CAL_BACKEND_DIR}/libecalbackendjmap.so
	${CAMEL_PROVIDER_DIR}/libcameljmap.so
	${CAMEL_PROVIDER_DIR}/libcameljmap.urls
	${EDS_REGISTRY_MODULE_DIR}/module-jmap-backend.so
	${EVOLUTION_MODULE_DIR}/module-jmap-configuration.so
)

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
