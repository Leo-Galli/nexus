project(
  'nexus',
  version: '0.1.0',
  meson_version: '>= 0.62',
  default_options: [
    'warning_level=2',
  ],
)

application_id = 'io.github.GITHUB_OWNER.Nexus'
gettext_package = meson.project_name()

i18n = import('i18n')
gnome = import('gnome')

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
localedir = prefix / get_option('localedir')
datadir = prefix / get_option('datadir')
pkgdatadir = datadir / meson.project_name()
iconsdir = datadir / 'icons'
podir = meson.project_source_root() / 'po'

# Fail fast with a clear message if the GNOME platform dependencies are
# missing, even though the actual Rust build links them through cargo
# (via pkg-config, same as these checks) rather than through Meson's own
# compiler driver.
dependency('glib-2.0', version: '>= 2.80')
dependency('gio-2.0', version: '>= 2.80')
dependency('gtk4', version: '>= 4.14')
dependency('libadwaita-1', version: '>= 1.5')

cargo = find_program('cargo')
blueprint_compiler = find_program('blueprint-compiler', required: true)

if get_option('profile') == 'development'
  profile = 'Devel'
  vcs_tag = run_command('git', 'rev-parse', '--short', 'HEAD', check: false).stdout().strip()
  if vcs_tag == ''
    version_suffix = '-devel'
  else
    version_suffix = '-devel-@0@'.format(vcs_tag)
  endif
else
  profile = ''
  version_suffix = ''
endif

subdir('data')
subdir('src')
subdir('po')

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
