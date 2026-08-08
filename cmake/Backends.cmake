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
