# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The install locations of the JMAP modules. Each host has its own module
# directory and naming convention, and reports the directory in its own
# pkg-config variable.
#
# Requires cmake/Rust.cmake for add_cargo_cdylib(), and pkg_check_variable()
# from the top-level CMakeLists.txt.

# Where evolution-addressbook-factory scans for backend modules.
pkg_check_modules(LIBEDATA_BOOK REQUIRED libedata-book-1.2>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(EDS_BOOK_BACKEND_DIR libedata-book-1.2 backenddir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(eds_book_prefix libedata-book-1.2 prefix)
	string(REGEX REPLACE "^${eds_book_prefix}" "${CMAKE_INSTALL_PREFIX}" EDS_BOOK_BACKEND_DIR "${EDS_BOOK_BACKEND_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP address book backend. The factory dlopens each module in its
# backend directory, then finds EBookBackendFactory subclasses among the new
# types. Thus only the directory must be correct. The name follows the
# in-tree EDS convention libebookbackend<name>.so, not the
# libjmap_backend_book_module.so that cargo builds.
#
# This crate is the `-module` companion, not the backend library. Only this
# cdylib holds `e_module_load` and `e_module_unload`. Each module in this
# repository must export the same pair, and two rlibs that defined them
# collided in a static link. See
# rust/crates/jmap-backend-collection-module/src/lib.rs.
add_cargo_cdylib(jmap_backend_book_module
	OUTPUT_NAME libebookbackendjmap.so
	DESTINATION ${EDS_BOOK_BACKEND_DIR}
	COMPONENT book-backend
	SYMBOLS e_module_load e_module_unload
	VERIFY_DESTINATION_FROM libedata-book-1.2 backenddir
)

# Where evolution-calendar-factory scans. A different pkg-config module
# reports a different directory. Thus both backends can have the name `jmap`
# and both can export `e_module_load`.
pkg_check_modules(LIBEDATA_CAL REQUIRED libedata-cal-2.0>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(EDS_CAL_BACKEND_DIR libedata-cal-2.0 backenddir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(eds_cal_prefix libedata-cal-2.0 prefix)
	string(REGEX REPLACE "^${eds_cal_prefix}" "${CMAKE_INSTALL_PREFIX}" EDS_CAL_BACKEND_DIR "${EDS_CAL_BACKEND_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP calendar backend, the same as the address book backend. The name
# follows the in-tree EDS convention libecalbackend<name>.so, not the
# libjmap_backend_cal_module.so that cargo builds. The crate is the `-module`
# companion that holds the entry points.
add_cargo_cdylib(jmap_backend_cal_module
	OUTPUT_NAME libecalbackendjmap.so
	DESTINATION ${EDS_CAL_BACKEND_DIR}
	COMPONENT cal-backend
	SYMBOLS e_module_load e_module_unload
	VERIFY_DESTINATION_FROM libedata-cal-2.0 backenddir
)

# Where Camel scans for mail providers: a third directory and a third
# pkg-config module. Camel does not dlopen each file. It reads the `.urls`
# file beside each object to find its protocols. It opens the object only
# when a client requests one of those protocols.
pkg_check_modules(CAMEL REQUIRED camel-1.2>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(CAMEL_PROVIDER_DIR camel-1.2 camel_providerdir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(camel_prefix camel-1.2 prefix)
	string(REGEX REPLACE "^${camel_prefix}" "${CMAKE_INSTALL_PREFIX}" CAMEL_PROVIDER_DIR "${CAMEL_PROVIDER_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP mail provider. libcamel<protocol>.so is the Camel convention.
# Camel derives the `.urls` name from the module name, so libcameljmap.urls
# must be beside libcameljmap.so. There is one entry point and no unload
# function, because Camel never closes a provider module.
add_cargo_cdylib(jmap_mail
	OUTPUT_NAME libcameljmap.so
	DESTINATION ${CAMEL_PROVIDER_DIR}
	COMPONENT camel-provider
	SYMBOLS camel_provider_module_init
	DATA ${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls
	VERIFY_DESTINATION_FROM camel-1.2 camel_providerdir
)

# Where evolution-source-registry scans: a fourth directory and a fourth
# pkg-config module. Of the four hosts, only this one is a single process for
# the full session, not one for each account. Thus its module entry point has
# two guards.
pkg_check_modules(LIBEBACKEND REQUIRED libebackend-1.2>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(EDS_REGISTRY_MODULE_DIR libebackend-1.2 moduledir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(eds_backend_prefix libebackend-1.2 prefix)
	string(REGEX REPLACE "^${eds_backend_prefix}" "${CMAKE_INSTALL_PREFIX}" EDS_REGISTRY_MODULE_DIR "${EDS_REGISTRY_MODULE_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP collection backend. It makes the mail, address book and calendar
# sources of one account, which the three modules above serve.
#
# Registry modules use the convention `module-*.so` (for example
# module-google-backend.so, module-cache-reaper.so). The registry dlopens each
# file in the directory, whatever its name. The convention is only for human
# readers. The `-backend` suffix makes this name different from
# module-jmap-configuration.so in the Evolution module directory.
add_cargo_cdylib(jmap_backend_collection_module
	OUTPUT_NAME module-jmap-backend.so
	DESTINATION ${EDS_REGISTRY_MODULE_DIR}
	COMPONENT collection-backend
	SYMBOLS e_module_load e_module_unload
	VERIFY_DESTINATION_FROM libebackend-1.2 moduledir
)

# The JMAP account configuration module. Of the five modules, only this one
# loads into Evolution, not into a data server process. EVOLUTION_MODULE_DIR
# is `moduledir` from evolution-shell-3.0. The top-level CMakeLists.txt sets
# it, because the example module also installs there.
#
# `module-*.so` again, and the shell dlopens each file in the directory,
# whatever its name. The `-configuration` suffix makes this name different
# from module-jmap-backend.so: a different module, directory and process.
# Without the suffix, a bug report cannot identify which of the two it means.
add_cargo_cdylib(jmap_config_module
	OUTPUT_NAME module-jmap-configuration.so
	DESTINATION ${EVOLUTION_MODULE_DIR}
	COMPONENT config-module
	SYMBOLS e_module_load e_module_unload
	VERIFY_DESTINATION_FROM evolution-shell-3.0 moduledir
)
