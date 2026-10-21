conf = configuration_data()
conf.set_quoted('APP_ID', application_id)
conf.set_quoted('VERSION', meson.project_version() + version_suffix)
conf.set_quoted('GETTEXT_PACKAGE', gettext_package)
conf.set_quoted('LOCALEDIR', localedir)
conf.set_quoted('PKGDATADIR', pkgdatadir)
conf.set_quoted('PROFILE', profile)
conf.set_quoted('RESOURCES_FILE', pkgdatadir / 'nexus.gresource')
configure_file(
  input: 'config.rs.in',
  output: 'config.rs',
  configuration: conf,
)
# `cargo build` reads the generated file from the *source* tree (Cargo
# does not know about Meson's separate build directory), so copy it next
# to the other source files; `src/config.rs` itself is gitignored.
run_command(
  'cp',
  meson.current_build_dir() / 'config.rs',
  meson.current_source_dir() / 'config.rs',
  check: true,
)

resources_source_root = meson.project_source_root() / 'resources'

blueprint_sources = files(
  '../resources/ui/window.blp',
  '../resources/ui/chat-view.blp',
  '../resources/ui/chat-message-row.blp',
  '../resources/ui/model-picker-row.blp',
  '../resources/ui/agents-view.blp',
  '../resources/ui/agent-session-row.blp',
  '../resources/ui/automations-view.blp',
  '../resources/ui/automation-editor.blp',
  '../resources/ui/stats-view.blp',
  '../resources/ui/preferences.blp',
  '../resources/ui/provider-row.blp',
  '../resources/ui/shortcuts.blp',
  '../resources/ui/empty-state.blp',
)

blueprints = custom_target(
  'blueprints',
  input: blueprint_sources,
  output: '.',
  command: [
    find_program('blueprint-compiler'),
    'batch-compile',
    '@OUTPUT@',
    resources_source_root,
    '@INPUT@',
  ],
)

nexus_resources = gnome.compile_resources(
  'nexus-resources',
  resources_source_root / 'nexus.gresource.xml',
  gresource_bundle: true,
  install: true,
  install_dir: pkgdatadir,
  source_dir: [meson.current_build_dir(), resources_source_root],
  dependencies: blueprints,
)

cargo_script = meson.project_source_root() / 'build-aux' / 'cargo.sh'
cargo_build_profile = get_option('profile') == 'development' ? 'debug' : 'release'

cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: bindir,
  depends: [nexus_resources, blueprints],
  command: [
    find_program('sh'),
    cargo_script,
    meson.project_build_root(),
    meson.project_source_root(),
    '@OUTPUT@',
    cargo_build_profile,
  ],
)
