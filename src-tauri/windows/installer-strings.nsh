; The installer hooks' own messages, in every installer language (FR-093).
;
; tauri.conf.json > bundle.windows.nsis.languages lists English and Spanish;
; Tauri translates its own pages, and these are the lines our hooks add (the
; takeover of a MaeclyPrint install, ADR-0009): the one question it may ask
; and the lines of the "Show details" log.
;
; Included by installer-hooks.nsh, which Tauri's template includes *before*
; it loads the languages, so the language ids are numeric (${LANG_ENGLISH}
; and ${LANG_SPANISH} do not exist yet): 1033 is English, 1034 Spanish. A
; LangString may be declared before its language is loaded.
;
; The file stays ASCII, like the hooks (NSIS reads a BOM-less include in the
; build machine's code page): every non-ASCII character is a ${U+XXXX}
; code point. Variables ($R1, $R2, $R5) expand when the line is shown.
; src-tauri/tests/installer_i18n.rs checks that every $(PLURY_*) the hooks
; use is declared here in both languages.

!define PLURY_LANG_EN 1033
!define PLURY_LANG_ES 1034

LangString PLURY_NOT_STARTED ${PLURY_LANG_EN} "not started"
LangString PLURY_NOT_STARTED ${PLURY_LANG_ES} "no se inici${U+00F3}"

LangString PLURY_PARK_LATE ${PLURY_LANG_EN} "Could not move this file aside; it stays until Explorer lets go of it:"
LangString PLURY_PARK_LATE ${PLURY_LANG_ES} "No se pudo apartar este archivo; se queda hasta que el Explorador lo libere:"

LangString PLURY_PARKED ${PLURY_LANG_EN} "In use by Explorer; moved aside and deleted at your next sign-in:"
LangString PLURY_PARKED ${PLURY_LANG_ES} "En uso por el Explorador; se ha apartado y se borrar${U+00E1} la pr${U+00F3}xima vez que inicies sesi${U+00F3}n:"

LangString PLURY_LEGACY_UNKNOWN_DIR ${PLURY_LANG_EN} "Cannot tell where ${PLURY_LEGACY_PRODUCT} is installed; leaving it in place"
LangString PLURY_LEGACY_UNKNOWN_DIR ${PLURY_LANG_ES} "No se sabe d${U+00F3}nde est${U+00E1} instalado ${PLURY_LEGACY_PRODUCT}; se deja como est${U+00E1}"

LangString PLURY_LEGACY_REMOVING ${PLURY_LANG_EN} "Removing the previous ${PLURY_LEGACY_PRODUCT} install in $R1"
LangString PLURY_LEGACY_REMOVING ${PLURY_LANG_ES} "Quitando la instalaci${U+00F3}n anterior de ${PLURY_LEGACY_PRODUCT} en $R1"

LangString PLURY_LEGACY_EXITED ${PLURY_LANG_EN} "${PLURY_LEGACY_PRODUCT} uninstaller exited with $R2"
LangString PLURY_LEGACY_EXITED ${PLURY_LANG_ES} "El desinstalador de ${PLURY_LEGACY_PRODUCT} termin${U+00F3} con $R2"

LangString PLURY_LEGACY_KEPT ${PLURY_LANG_EN} "${PLURY_LEGACY_PRODUCT} was not removed; it stays in Apps & features"
LangString PLURY_LEGACY_KEPT ${PLURY_LANG_ES} "${PLURY_LEGACY_PRODUCT} no se quit${U+00F3}; sigue en Configuraci${U+00F3}n ${U+203A} Aplicaciones"

LangString PLURY_LEGACY_STALE ${PLURY_LANG_EN} "${PLURY_LEGACY_PRODUCT} uninstaller not found in $R1; removing its stale registration"
LangString PLURY_LEGACY_STALE ${PLURY_LANG_ES} "No se encontr${U+00F3} el desinstalador de ${PLURY_LEGACY_PRODUCT} en $R1; se quita su registro hu${U+00E9}rfano"

LangString PLURY_MSI_NOT_MSI ${PLURY_LANG_EN} "A per-machine ${PLURY_LEGACY_PRODUCT} install ($R5) was found but is not an MSI; remove it from Apps & features"
LangString PLURY_MSI_NOT_MSI ${PLURY_LANG_ES} "Hay una instalaci${U+00F3}n de ${PLURY_LEGACY_PRODUCT} para todos los usuarios ($R5) que no es un MSI; desinst${U+00E1}lala desde Configuraci${U+00F3}n ${U+203A} Aplicaciones"

LangString PLURY_MSI_FOUND ${PLURY_LANG_EN} "Found a per-machine ${PLURY_LEGACY_PRODUCT} install ($R5)"
LangString PLURY_MSI_FOUND ${PLURY_LANG_ES} "Hay una instalaci${U+00F3}n de ${PLURY_LEGACY_PRODUCT} para todos los usuarios ($R5)"

LangString PLURY_MSI_ASK ${PLURY_LANG_EN} "${PLURY_LEGACY_PRODUCT} is also installed for all users of this computer. Remove it now? Windows will ask for administrator approval.$\n$\nYour settings, profiles and queue are kept either way."
LangString PLURY_MSI_ASK ${PLURY_LANG_ES} "${PLURY_LEGACY_PRODUCT} tambi${U+00E9}n est${U+00E1} instalado para todos los usuarios de este equipo. ${U+00BF}Quieres quitarlo ahora? Windows pedir${U+00E1} la aprobaci${U+00F3}n de un administrador.$\n$\nTu configuraci${U+00F3}n, tus perfiles y tu cola se conservan en cualquier caso."

LangString PLURY_MSI_LEFT ${PLURY_LANG_EN} "Left the per-machine ${PLURY_LEGACY_PRODUCT} install in place; remove it from Apps & features"
LangString PLURY_MSI_LEFT ${PLURY_LANG_ES} "Se dej${U+00F3} la instalaci${U+00F3}n de ${PLURY_LEGACY_PRODUCT} para todos los usuarios; desinst${U+00E1}lala desde Configuraci${U+00F3}n ${U+203A} Aplicaciones"

LangString PLURY_MSI_EXITED ${PLURY_LANG_EN} "${PLURY_LEGACY_PRODUCT} (per machine) uninstall exited with $R2"
LangString PLURY_MSI_EXITED ${PLURY_LANG_ES} "La desinstalaci${U+00F3}n de ${PLURY_LEGACY_PRODUCT} (para todos los usuarios) termin${U+00F3} con $R2"

LangString PLURY_LEGACY_PROFILE_REMOVING ${PLURY_LANG_EN} "Removing the unused ${PLURY_LEGACY_PRODUCT} web view profile"
LangString PLURY_LEGACY_PROFILE_REMOVING ${PLURY_LANG_ES} "Quitando el perfil de vista web de ${PLURY_LEGACY_PRODUCT}, que ya no se usa"

LangString PLURY_SHELLEXT_MISSING ${PLURY_LANG_EN} "This installer carries no $R8 context-menu DLL; the Explorer menu is not registered"
LangString PLURY_SHELLEXT_MISSING ${PLURY_LANG_ES} "Este instalador no trae la DLL del men${U+00FA} contextual para $R8; el men${U+00FA} del Explorador no se registra"

LangString PLURY_SHELLEXT_REGISTERING ${PLURY_LANG_EN} "Registering the $R8 Explorer context menu"
LangString PLURY_SHELLEXT_REGISTERING ${PLURY_LANG_ES} "Registrando el men${U+00FA} contextual del Explorador ($R8)"

LangString PLURY_BRIDGE_REGISTERING ${PLURY_LANG_EN} "Registering the browser bridge for PluryFact (Chrome, Edge, Brave, Chromium)"
LangString PLURY_BRIDGE_REGISTERING ${PLURY_LANG_ES} "Registrando el puente con el navegador para PluryFact (Chrome, Edge, Brave, Chromium)"

LangString PLURY_BRIDGE_MISSING ${PLURY_LANG_EN} "This installer carries no browser bridge; PluryFact cannot print through Pluryprint"
LangString PLURY_BRIDGE_MISSING ${PLURY_LANG_ES} "Este instalador no trae el puente con el navegador; PluryFact no podr${U+00E1} imprimir con Pluryprint"

LangString PLURY_BRIDGE_WRITE_FAILED ${PLURY_LANG_EN} "Could not write the browser bridge's manifest; run pluryprint-cli bridge install to try again"
LangString PLURY_BRIDGE_WRITE_FAILED ${PLURY_LANG_ES} "No se pudo escribir el manifiesto del puente con el navegador; ejecuta pluryprint-cli bridge install para reintentarlo"
