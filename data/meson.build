# data/meson.build
#
# Installs the desktop entry, AppStream metainfo, GSettings schema, and
# hicolor icons for Nexus.
#
# Expected to be included via `subdir('data')` from the top-level
# meson.build, which must already have defined, before this subdir() call:
#
#   i18n            - the i18n module object, i.e. `i18n = import('i18n')`
#   application_id  - the application ID string, e.g.
#                      'io.github.GITHUB_OWNER.Nexus'
#
# This file does not call gnome.post_install(glib_compile_schemas: true)
# itself; that is expected to be called once, in the top-level meson.build,
# alongside any other gnome.post_install() housekeeping (icon cache update,
# desktop database update) so it is not duplicated across subdirectories.

desktop_file = i18n.merge_file(
  input: application_id + '.desktop.in',
  output: application_id + '.desktop',
  type: 'desktop',
  po_dir: '../po',
  install: true,
  install_dir: get_option('datadir') / 'applications',
)

desktop_utils = find_program('desktop-file-validate', required: false)
if desktop_utils.found()
  test(
    'validate-desktop',
    desktop_utils,
    args: [desktop_file],
  )
endif

appstream_file = i18n.merge_file(
  input: application_id + '.metainfo.xml.in',
  output: application_id + '.metainfo.xml',
  po_dir: '../po',
  install: true,
  install_dir: get_option('datadir') / 'metainfo',
)

appstream_util = find_program('appstreamcli', required: false)
if appstream_util.found()
  test(
    'validate-appstream',
    appstream_util,
    args: ['validate', '--no-net', '--pedantic', appstream_file],
  )
endif

install_data(
  application_id + '.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

compile_schemas = find_program('glib-compile-schemas', required: false)
if compile_schemas.found()
  test(
    'validate-schema',
    compile_schemas,
    args: ['--strict', '--dry-run', meson.current_source_dir()],
  )
endif

install_data(
  'icons' / 'hicolor' / 'scalable' / 'apps' / (application_id + '.svg'),
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
)

install_data(
  'icons' / 'hicolor' / 'symbolic' / 'apps' / (application_id + '-symbolic.svg'),
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'symbolic' / 'apps',
)
