# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# add_translations(): compiles a translator's `.po` into the `.mo` that
# gettext opens at run time.
#
# This file is separate from cmake/Translations.cmake (the caller). Thus the
# fixture project of cmake/tests/check-translations.cmake can include the
# function without this project's catalogues and without a second registration
# of that test.

# Compiles each language in <PO_DIR>/LINGUAS and installs it where gettext
# looks: <DESTINATION>/<language>/LC_MESSAGES/<DOMAIN>.mo
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
# Packaging uses OUTPUT_VARIABLE as the list of package files. Thus it must
# name exactly the files that the install rules produce, and the caller does
# not calculate the layout again.
function(add_translations)
	cmake_parse_arguments(_arg ""
		"PO_DIR;DOMAIN;DESTINATION;COMPONENT;COMPILER;COMPILER_TARGET;TARGET;OUTPUT_VARIABLE"
		"" ${ARGN})

	foreach(_required PO_DIR DOMAIN DESTINATION COMPONENT COMPILER TARGET)
		if(NOT _arg_${_required})
			message(FATAL_ERROR "add_translations(): ${_required} is required and must not be empty")
		endif()
	endforeach()

	# Same reason as in add_cargo_cdylib(): these directories come from
	# variables that report "unset" as an empty string. A relative destination
	# installs into the prefix root, whatever it is.
	if(NOT IS_ABSOLUTE "${_arg_DESTINATION}")
		message(FATAL_ERROR "add_translations(): DESTINATION '${_arg_DESTINATION}' is not an absolute path")
	endif()

	set(_linguas "${_arg_PO_DIR}/LINGUAS")
	if(NOT EXISTS "${_linguas}")
		message(FATAL_ERROR "add_translations(): no LINGUAS in ${_arg_PO_DIR}")
	endif()

	# To add a language, a translator edits only LINGUAS. CMake must configure
	# again after that edit. If not, the new language builds only in a new
	# build tree.
	set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_linguas}")

	# gettext format: one language per line and `#` comments. A line can also
	# hold several codes, so split on whitespace.
	#
	# ENCODING UTF-8 is necessary. Without it, file(STRINGS) reads the file
	# as strings(1) does: a non-ASCII byte is binary and *breaks the line*.
	# A comment with an em dash then becomes two lines. The second line has
	# no `#`, so it is read as a list of languages. This project's own
	# po/LINGUAS has such a comment.
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
		# A language code becomes a directory name in a path. Refuse other
		# values: a `..` or a slash in LINGUAS installs outside the locale
		# directory.
		if(NOT _lang MATCHES "^[A-Za-z][A-Za-z0-9_@.-]*$")
			message(FATAL_ERROR
				"add_translations(): '${_lang}' in ${_linguas} is not a language code")
		endif()
		# list(FIND), not IN_LIST: a project that includes this file has a
		# cmake_minimum_required() older than CMP0057. There, IN_LIST is not
		# an operator.
		list(FIND _seen "${_lang}" _already)
		if(NOT _already EQUAL -1)
			message(FATAL_ERROR "add_translations(): ${_linguas} names ${_lang} twice")
		endif()
		list(APPEND _seen "${_lang}")

		set(_po "${_arg_PO_DIR}/${_lang}.po")
		# Stop, do not skip. If a listed language has no catalogue, the
		# build ships one language fewer than it promises. From outside, a
		# skipped language looks the same as a translated one.
		if(NOT EXISTS "${_po}")
			message(FATAL_ERROR
				"add_translations(): ${_linguas} names ${_lang}, but there is no ${_po}")
		endif()

		# Build in the install layout, so the compiled file and the installed
		# file always have the same name and relative location.
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

	# ALL, and always created: the catalogues are files, not CMake targets.
	# Without this target, nothing builds them and `cmake --install` copies a
	# file that does not exist. With no languages, the target is empty: the
	# build installs no catalogue, and that is not a failure.
	add_custom_target(${_arg_TARGET} ALL DEPENDS ${_outputs})

	if(_arg_OUTPUT_VARIABLE)
		set(${_arg_OUTPUT_VARIABLE} "${_installed}" PARENT_SCOPE)
	endif()
endfunction()
