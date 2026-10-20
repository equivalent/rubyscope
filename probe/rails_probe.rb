# Prints facts from a booted Rails app as JSON, for rubyscope.
#
# Run it in the app folder:
#
#   bin/rails runner /path/to/rails_probe.rb probe.json
#
# It reads only. It eager loads the app, then writes:
#
# - the Zeitwerk autoload roots with their namespaces, and the collapsed
#   and ignored folders,
# - the real source location of every named constant and method.
#
# rubyscope uses the autoload roots for its file rules, and
# `rubyscope-cli check` compares the locations with its own index.

require "json"

out_path = ARGV.fetch(0, "probe.json")

loader = Rails.autoloaders.main

# A full eager load stops at the first error. Some apps read the database
# when a class loads (for example to check its columns), which fails when
# the database is not running. Then load each constant on its own and skip
# the ones that fail.
skipped = []
begin
  Rails.application.eager_load!
rescue StandardError, ScriptError => e
  warn "rails_probe: eager_load! failed (#{e.class}: #{e.message.lines.first&.strip}), loading constants one by one"
  Rails.autoloaders.each do |l|
    l.all_expected_cpaths.each_value do |cpath|
      Object.const_get(cpath)
    rescue StandardError, ScriptError
      skipped << cpath
    end
  end
end
name_of = Module.instance_method(:name)

autoload = {
  roots: loader.dirs(namespaces: true).map { |dir, ns| {dir:, namespace: ns.name} },
  collapse: loader.send(:collapse_dirs).to_a,
  ignore: loader.send(:ignored_paths).to_a
}

constants = {}
methods = []

ObjectSpace.each_object(Module) do |mod|
  name = name_of.bind_call(mod)
  next unless name.is_a?(String) && name.match?(/\A[A-Z]\w*(::[A-Z]\w*)*\z/)

  begin
    file, line = Object.const_source_location(name)
    constants[name] = [file, line] if file && !file.empty?
  rescue StandardError, ScriptError
    # A constant that was removed or renamed after it was named, or one
    # whose file failed to load above.
  end

  [[mod, false], [mod.singleton_class, true]].each do |m, singleton|
    names = m.instance_methods(false) + m.private_instance_methods(false)
    names.each do |meth|
      um = m.instance_method(meth)
      next unless um.owner == m

      file, line = um.source_location
      next if file.nil? || file.start_with?("<internal:", "(eval")

      methods << [name, singleton, meth.to_s, file, line]
    rescue StandardError
      next
    end
  end
end

File.write(out_path, JSON.generate({root: Rails.root.to_s, autoload:, skipped:, constants:, methods:}))
warn "rails_probe: #{constants.size} constants, #{methods.size} methods, #{skipped.size} constants skipped, written to #{out_path}"
