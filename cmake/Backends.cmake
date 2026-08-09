# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Where the JMAP backend modules are installed. Each host — the address book
# factory, the calendar factory, Camel — has its own module directory and its
# own naming convention, and each reports the directory through its own
# pkg-config variable. Requires cmake/Rust.cmake for add_cargo_cdylib() and
# the pkg_check_variable() defined by the top-level CMakeLists.txt.

# Where evolution-addressbook-factory scans for backend modules.
pkg_check_modules(LIBEDATA_BOOK REQUIRED libedata-book-1.2>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(EDS_BOOK_BACKEND_DIR libedata-book-1.2 backenddir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(eds_book_prefix libedata-book-1.2 prefix)
	string(REGEX REPLACE "^${eds_book_prefix}" "${CMAKE_INSTALL_PREFIX}" EDS_BOOK_BACKEND_DIR "${EDS_BOOK_BACKEND_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP address book backend. The factory dlopens every module in its
# backend directory and then looks for EBookBackendFactory subclasses among
# the types that appeared, so the directory is what has to be right; the
# libebookbackend<name>.so spelling is the convention every in-tree EDS
# backend follows, and not the libjmap_backend_book.so cargo builds.
add_cargo_cdylib(jmap_backend_book
	OUTPUT_NAME libebookbackendjmap.so
	DESTINATION ${EDS_BOOK_BACKEND_DIR}
	COMPONENT book-backend
	SYMBOLS e_module_load e_module_unload
	VERIFY_DESTINATION_FROM libedata-book-1.2 backenddir
)

# Where evolution-calendar-factory scans. A different directory reported by a
# different pkg-config module, which is the whole reason the two backends can
# both be called `jmap` and both export `e_module_load`.
pkg_check_modules(LIBEDATA_CAL REQUIRED libedata-cal-2.0>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(EDS_CAL_BACKEND_DIR libedata-cal-2.0 backenddir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(eds_cal_prefix libedata-cal-2.0 prefix)
	string(REGEX REPLACE "^${eds_cal_prefix}" "${CMAKE_INSTALL_PREFIX}" EDS_CAL_BACKEND_DIR "${EDS_CAL_BACKEND_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP calendar backend. Same story as the address book one directory over,
# down to the naming convention: libecalbackend<name>.so is what every in-tree
# EDS calendar backend is called, and not the libjmap_backend_cal.so cargo
# builds.
add_cargo_cdylib(jmap_backend_cal
	OUTPUT_NAME libecalbackendjmap.so
	DESTINATION ${EDS_CAL_BACKEND_DIR}
	COMPONENT cal-backend
	SYMBOLS e_module_load e_module_unload
	VERIFY_DESTINATION_FROM libedata-cal-2.0 backenddir
)

# Where Camel scans for mail providers. A third directory, a third pkg-config
# module — and a different loading story: Camel does not dlopen everything it
# finds. It reads the `.urls` file beside each object to learn which protocols
# that object claims, and opens the object only when one of them is asked for.
pkg_check_modules(CAMEL REQUIRED camel-1.2>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(CAMEL_PROVIDER_DIR camel-1.2 camel_providerdir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(camel_prefix camel-1.2 prefix)
	string(REGEX REPLACE "^${camel_prefix}" "${CMAKE_INSTALL_PREFIX}" CAMEL_PROVIDER_DIR "${CAMEL_PROVIDER_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP mail provider. libcamel<protocol>.so is Camel's own convention and
# the name the `.urls` file has to match — Camel derives one from the other, so
# libcameljmap.urls beside libcameljmap.so is not a style choice. One entry
# point, and no unload counterpart: Camel never closes a provider module.
add_cargo_cdylib(jmap_mail
	OUTPUT_NAME libcameljmap.so
	DESTINATION ${CAMEL_PROVIDER_DIR}
	COMPONENT camel-provider
	SYMBOLS camel_provider_module_init
	DATA ${CMAKE_SOURCE_DIR}/rust/crates/jmap-mail/libcameljmap.urls
	VERIFY_DESTINATION_FROM camel-1.2 camel_providerdir
)

# Where evolution-source-registry scans. A fourth directory and a fourth
# pkg-config module — and the one host of the four that is a single process for
# the whole session rather than one per account, which is why the module entry
# point in it is guarded twice over.
pkg_check_modules(LIBEBACKEND REQUIRED libebackend-1.2>=${REQUIRE_EVOLUTION_VERSION})
pkg_check_variable(EDS_REGISTRY_MODULE_DIR libebackend-1.2 moduledir)

if(FORCE_INSTALL_PREFIX)
	pkg_check_variable(eds_backend_prefix libebackend-1.2 prefix)
	string(REGEX REPLACE "^${eds_backend_prefix}" "${CMAKE_INSTALL_PREFIX}" EDS_REGISTRY_MODULE_DIR "${EDS_REGISTRY_MODULE_DIR}")
endif(FORCE_INSTALL_PREFIX)

# The JMAP collection backend, which is what turns one account into the mail,
# address book and calendar sources the three modules above serve. `module-*.so`
# is the convention every registry module follows — module-google-backend.so,
# module-cache-reaper.so — and unlike the book and calendar backends the name is
# not derived from anything: the registry dlopens every file in the directory
# regardless of what it is called. Following the convention is for the human
# reading the directory, and the `-backend` suffix distinguishes this from M7's
# module-jmap-configuration.so, which is Evolution's module directory over.
add_cargo_cdylib(jmap_backend_collection
	OUTPUT_NAME module-jmap-backend.so
	DESTINATION ${EDS_REGISTRY_MODULE_DIR}
	COMPONENT collection-backend
	SYMBOLS e_module_load e_module_unload
	VERIFY_DESTINATION_FROM libebackend-1.2 moduledir
)
