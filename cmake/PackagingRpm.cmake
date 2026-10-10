# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later

include_guard(GLOBAL)

# Only add RPM to the generator list cpack builds by default
# (`cmake --build <dir> --target package`, which is what the release
# workflow runs) when rpmbuild is actually installed. Appending it
# unconditionally broke that exact command on the release image, which
# has no rpmbuild: CPack's RPM generator aborts the whole `cpack` run with
# "RPM package requires rpmbuild executable" even though nothing asked for
# an RPM, taking the .deb down with it. `ci/rpm.sh` is the documented way
# to build the RPM deliberately, in a Fedora container that has rpmbuild.
find_program(RPMBUILD_EXECUTABLE rpmbuild)
if(RPMBUILD_EXECUTABLE AND NOT "RPM" IN_LIST CPACK_GENERATOR)
	list(APPEND CPACK_GENERATOR RPM)
endif()

set(CPACK_RPM_COMPONENT_INSTALL ON)
set(CPACK_RPM_MAIN_COMPONENT "collection-backend")

set(CPACK_RPM_FILE_NAME "RPM-DEFAULT")

pkg_check_variable(EVOLUTION_PRIVATE_LIB_DIR evolution-shell-3.0 privlibdir)
if(NOT EVOLUTION_PRIVATE_LIB_DIR)
	message(FATAL_ERROR
		"evolution-shell-3.0 reports no privlibdir; RPM dependency scanning "
		"cannot validate private Evolution library linkage")
endif()

set(CPACK_RPM_EXCLUDE_FROM_AUTO_FILELIST_ADDITION
	"${EVOLUTION_PRIVATE_LIB_DIR}"
)

set(CPACK_RPM_PACKAGE_LICENSE "GPL-3.0-or-later")
set(CPACK_RESOURCE_FILE_LICENSE "${CMAKE_SOURCE_DIR}/LICENSES/GPL-3.0-or-later.txt")

set(CPACK_RPM_PACKAGE_GROUP "Applications/Productivity")
