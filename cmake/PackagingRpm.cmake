# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later

include_guard(GLOBAL)

if(NOT "RPM" IN_LIST CPACK_GENERATOR)
	list(APPEND CPACK_GENERATOR RPM)
endif()

set(CPACK_RPM_COMPONENT_INSTALL ON)
set(CPACK_RPM_MAIN_COMPONENT "collection-backend")

set(CPACK_RPM_FILE_NAME "RPM-DEFAULT")
set(CPACK_RPM_PACKAGE_RELOCATABLE OFF)
set(CPACK_RPM_PACKAGE_DESCRIPTION
	"Backends that let Evolution and evolution-data-server speak JMAP."
)

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

set(CPACK_RPM_USER_FILELIST
	"%attr(-,root,root) ${EDS_BOOK_BACKEND_DIR}/libebookbackendjmap.so"
	"%attr(-,root,root) ${EDS_CAL_BACKEND_DIR}/libecalbackendjmap.so"
	"%attr(-,root,root) ${CAMEL_PROVIDER_DIR}/libcameljmap.so"
	"%attr(-,root,root) ${CAMEL_PROVIDER_DIR}/libcameljmap.urls"
	"%attr(-,root,root) ${EDS_REGISTRY_MODULE_DIR}/module-jmap-backend.so"
	"%attr(-,root,root) ${EVOLUTION_MODULE_DIR}/module-jmap-configuration.so"
	"%attr(-,root,root) /usr/share/doc/${PACKAGE_NAME}/changelog.gz"
	"%attr(-,root,root) /usr/share/doc/${PACKAGE_NAME}/copyright"
	"%attr(-,root,root) /usr/share/doc/${PACKAGE_NAME}/third-party-notices"
	"%attr(-,root,root) /usr/share/lintian/overrides/${PACKAGE_NAME}"
)

if(INSTALLED_CATALOGUES)
	foreach(_catalogue IN LISTS INSTALLED_CATALOGUES)
		list(APPEND CPACK_RPM_USER_FILELIST "%attr(-,root,root) ${_catalogue}")
	endforeach()
endif()
