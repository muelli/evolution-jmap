# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# add_translations() — the build step between a translator's `.po` and the
# `.mo` gettext opens at run time.
#
# Kept apart from cmake/Translations.cmake, which calls it, so that the
# fixture project cmake/tests/check-translations.cmake builds can include the
# function under test without inheriting this project's own catalogues or
# re-registering that test.

# Compiles every language named in <PO_DIR>/LINGUAS and installs each one
# where gettext looks for it: <DESTINATION>/<language>/LC_MESSAGES/<DOMAIN>.mo
#
#   add_translations(
#       PO_DIR      <directory holding LINGUAS and the .po files>
#       DOMAIN      <gettext domain; also the catalogue's basename>
#       DESTINATION <absolute locale directory to install under>
#       COMPONENT   <install component>
#       COMPILER    <po-compile executable>
#       TARGET      <name for the target that builds the catalogues>
#       [COMPILER_TARGET <target that produces COMPILER>]
#       [OUTPUT_VARIABLE <var>]  # parent-scope list of installed paths
#   )
#
# OUTPUT_VARIABLE is what packaging declares the package contains, so it has
# to name exactly the files the install rules produce — the caller must not
# have to re-derive the layout this function chose.
function(add_translations)
	cmake_parse_arguments(_arg ""
		"PO_DIR;DOMAIN;DESTINATION;COMPONENT;COMPILER;COMPILER_TARGET;TARGET;OUTPUT_VARIABLE"
		"" ${ARGN})

	foreach(_required PO_DIR DOMAIN DESTINATION COMPONENT COMPILER TARGET)
		if(NOT _arg_${_required})
			message(FATAL_ERROR "add_translations(): ${_required} is required and must not be empty")
		endif()
	endforeach()

	# Same reasoning as add_cargo_cdylib()'s: these directories come from
	# variables that report "unset" as the empty string, and a relative
	# destination would install into whatever the prefix root happens to be.
	if(NOT IS_ABSOLUTE "${_arg_DESTINATION}")
		message(FATAL_ERROR "add_translations(): DESTINATION '${_arg_DESTINATION}' is not an absolute path")
	endif()

	set(_linguas "${_arg_PO_DIR}/LINGUAS")
	if(NOT EXISTS "${_linguas}")
		message(FATAL_ERROR "add_translations(): no LINGUAS in ${_arg_PO_DIR}")
	endif()

	# Adding a language is editing LINGUAS, and nothing else — so CMake has
	# to notice that edit and re-run, or the new language builds only after
	# someone deletes the build tree.
	set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_linguas}")

	# gettext's own format: one language per line, `#` comments, and the
	# codes may equally be written several to a line, so split on whitespace
	# rather than trusting the layout.
	#
	# ENCODING UTF-8 is not decoration. Without it file(STRINGS) reads the
	# file the way strings(1) does — non-ASCII bytes are treated as binary
	# and *break the line there* — so a comment holding an em dash arrives
	# as two lines, the second of which no longer starts with a `#` and is
	# read as a list of languages. That is exactly what this project's own
	# po/LINGUAS holds.
	file(STRINGS "${_linguas}" _lines ENCODING UTF-8)
	set(_languages)
	foreach(_line IN LISTS _lines)
		string(REGEX REPLACE "#.*$" "" _line "${_line}")
		string(STRIP "${_line}" _line)
		if(_line STREQUAL "")
			continue()
		endif()
		string(REGEX REPLACE "[ \t]+" ";" _line_languages "${_line}")
		list(APPEND _languages ${_line_languages})
	endforeach()

	set(_outputs)
	set(_installed)
	set(_seen)
	foreach(_lang IN LISTS _languages)
		# A language code becomes a directory name and part of a path, so
		# refuse anything that is not one — a `..` or a slash in LINGUAS
		# would otherwise install outside the locale directory entirely.
		if(NOT _lang MATCHES "^[A-Za-z][A-Za-z0-9_@.-]*$")
			message(FATAL_ERROR
				"add_translations(): '${_lang}' in ${_linguas} is not a language code")
		endif()
		# list(FIND) rather than IN_LIST: this file is included by a project
		# whose cmake_minimum_required() predates CMP0057, where IN_LIST is
		# not an operator at all.
		list(FIND _seen "${_lang}" _already)
		if(NOT _already EQUAL -1)
			message(FATAL_ERROR "add_translations(): ${_linguas} names ${_lang} twice")
		endif()
		list(APPEND _seen "${_lang}")

		set(_po "${_arg_PO_DIR}/${_lang}.po")
		# Stop rather than skip. A language listed with no catalogue is a
		# build that ships one language fewer than it promises, and a
		# skipped one looks from the outside exactly like a translated one.
		if(NOT EXISTS "${_po}")
			message(FATAL_ERROR
				"add_translations(): ${_linguas} names ${_lang}, but there is no ${_po}")
		endif()

		# Built in the install layout so that what is compiled and what is
		# installed cannot drift apart in name or in place.
		set(_mo "${CMAKE_CURRENT_BINARY_DIR}/translations/${_lang}/LC_MESSAGES/${_arg_DOMAIN}.mo")
		set(_depends "${_po}")
		if(_arg_COMPILER_TARGET)
			list(APPEND _depends "${_arg_COMPILER_TARGET}")
		else()
			list(APPEND _depends "${_arg_COMPILER}")
		endif()

		get_filename_component(_mo_dir "${_mo}" DIRECTORY)
		add_custom_command(
			OUTPUT "${_mo}"
			COMMAND ${CMAKE_COMMAND} -E make_directory "${_mo_dir}"
			COMMAND "${_arg_COMPILER}" "${_po}" "${_mo}"
			DEPENDS ${_depends}
			COMMENT "Compiling ${_lang}.po"
			VERBATIM
		)
		list(APPEND _outputs "${_mo}")

		install(FILES "${_mo}"
			DESTINATION "${_arg_DESTINATION}/${_lang}/LC_MESSAGES"
			COMPONENT "${_arg_COMPONENT}"
		)
		list(APPEND _installed "${_arg_DESTINATION}/${_lang}/LC_MESSAGES/${_arg_DOMAIN}.mo")
	endforeach()

	# ALL, and unconditionally created: the catalogues are files rather than
	# CMake targets, so nothing else would build them and `cmake --install`
	# would copy a file that does not exist. With no languages the target is
	# empty, which is a build that installs no catalogue — not a failure.
	add_custom_target(${_arg_TARGET} ALL DEPENDS ${_outputs})

	if(_arg_OUTPUT_VARIABLE)
		set(${_arg_OUTPUT_VARIABLE} "${_installed}" PARENT_SCOPE)
	endif()
endfunction()
