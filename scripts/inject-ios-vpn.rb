#!/usr/bin/env ruby
# frozen_string_literal: true

# 将 iOS VPN 网络扩展注入 `tauri ios init` 生成的工程。
#
# gen/apple 由 CLI 现生成，所有定制必须在 init 之后经本脚本幂等注入：
#   1. 拷贝 PacketTunnelProvider.swift / Info.plist / bridging header / entitlements
#   2. 新建 PrismVPN app-extension target（链接 libprismkernel.a，打包 geo 数据库）
#   3. 主 App target 加入 PrismTunnelController.swift 并链接 NetworkExtension framework
#   4. 主 App 依赖扩展并添加 Embed Foundation Extensions 拷贝阶段
require 'xcodeproj'
require 'json'
require 'fileutils'

ROOT = File.expand_path('..', __dir__)
GEN_APPLE = File.join(ROOT, 'src-tauri', 'gen', 'apple')
KERNEL_DIR = File.join(ROOT, 'src-tauri', 'ios-kernel')
EXT_NAME = 'PrismVPN'

def inject
  project_path = Dir[File.join(GEN_APPLE, '*.xcodeproj')].first
  abort "[inject-ios-vpn] #{GEN_APPLE} 不存在，请先执行 npx tauri ios init" unless project_path

  project = Xcodeproj::Project.open(project_path)

  if project.targets.any? { |t| t.name == EXT_NAME }
    puts '[inject-ios-vpn] already injected, skip'
    return
  end

  app_target = project.targets.find { |t| t.product_type == 'com.apple.product-type.application' }
  abort '[inject-ios-vpn] 未找到主 App target' unless app_target

  app_bundle_id = app_target.build_configurations.first.build_settings['PRODUCT_BUNDLE_IDENTIFIER'] || 'com.prism.proxy'
  version = JSON.parse(File.read(File.join(ROOT, 'src-tauri', 'tauri.conf.json')))['version']

  # ---- 1. 拷贝扩展源文件 ----
  ext_dir = File.join(GEN_APPLE, EXT_NAME)
  FileUtils.mkdir_p(ext_dir)
  src_dir = File.join(ROOT, 'src-tauri', 'mobile-src', 'ios', 'PrismVPN')
  Dir[File.join(src_dir, '*')].each { |f| FileUtils.cp(f, ext_dir) }
  FileUtils.cp(
    File.join(ROOT, 'src-tauri', 'mobile-src', 'ios', 'App', 'PrismTunnelController.swift'),
    GEN_APPLE
  )

  # ---- 2. 创建扩展 target ----
  ext_target = project.new_target(:app_extension, EXT_NAME, :ios, '14.0')

  group = project.main_group.new_group(EXT_NAME, EXT_NAME)
  swift_ref = group.new_file(File.join(ext_dir, 'PacketTunnelProvider.swift'))
  ext_target.add_file_references([swift_ref])

  # geo 数据库随扩展包分发（扩展进程内运行内核，直接读自身 Bundle）
  %w[geoip.db geosite.db].each do |db|
    src = File.join(ROOT, 'src-tauri', 'binaries', db)
    abort "[inject-ios-vpn] 缺少 #{src}（先执行 fetch-kernel.cjs --geo-only）" unless File.exist?(src)
    FileUtils.cp(src, File.join(ext_dir, db))
    ref = group.new_file(File.join(ext_dir, db))
    # PBXResourcesBuildPhase 没有 add_file 方法（CI 实证 NoMethodError），
    # 构建阶段加文件的正确 API 是 add_file_reference
    ext_target.resources_build_phase.add_file_reference(ref)
  end
  group.new_file(File.join(ext_dir, 'Info.plist'))

  common_settings = {
    'PRODUCT_NAME' => EXT_NAME,
    'PRODUCT_BUNDLE_IDENTIFIER' => "#{app_bundle_id}.#{EXT_NAME}",
    'INFOPLIST_FILE' => "#{EXT_NAME}/Info.plist",
    'GENERATE_INFOPLIST_FILE' => 'NO',
    'CODE_SIGN_ENTITLEMENTS' => "#{EXT_NAME}/PrismVPN.entitlements.plist",
    'PRODUCT_MODULE_NAME' => EXT_NAME,
    'SWIFT_VERSION' => '5.0',
    'SWIFT_OBJC_BRIDGING_HEADER' => "#{EXT_NAME}/PrismVPN-Bridging-Header.h",
    'CLANG_ENABLE_MODULES' => 'YES',
    'TARGETED_DEVICE_FAMILY' => '1,2',
    'SKIP_INSTALL' => 'YES',
    'MARKETING_VERSION' => version.to_s,
    'CURRENT_PROJECT_VERSION' => '1',
    # Go c-archive 头文件与静态库
    'HEADER_SEARCH_PATHS' => "$(inherited)\n\"#{KERNEL_DIR}\"",
    'LIBRARY_SEARCH_PATHS' => "$(inherited)\n\"#{KERNEL_DIR}\"",
    # 与 build.rs 主 App 链接一致的内核依赖
    'OTHER_LDFLAGS' => '$(inherited) -lprismkernel -lresolv -lz ' \
                       '-framework Security -framework Foundation ' \
                       '-framework CoreFoundation -framework SystemConfiguration',
    'LD_RUNPATH_SEARCH_PATHS' => '$(inherited) @executable_path/Frameworks @executable_path/../../Frameworks',
  }
  ext_target.build_configurations.each do |cfg|
    cfg.build_settings.merge!(common_settings)
  end

  # ---- 3. 主 App：隧道控制器 + NetworkExtension 框架 ----
  ctrl_ref = project.main_group.new_file(File.join(GEN_APPLE, 'PrismTunnelController.swift'))
  app_target.add_file_references([ctrl_ref])

  app_target.build_configurations.each do |cfg|
    flags = cfg.build_settings['OTHER_LDFLAGS'] || '$(inherited)'
    unless flags.include?('NetworkExtension')
      cfg.build_settings['OTHER_LDFLAGS'] = "#{flags} -framework Network -framework NetworkExtension"
    end
  end

  # ---- 4. 依赖 + 嵌入 ----
  app_target.add_dependency(ext_target)
  embed = app_target.new_copy_files_build_phase('Embed Foundation Extensions')
  embed.dst_subfolder_spec = '13' # PlugIns
  embed.add_file_reference(ext_target.product_reference)

  project.save
  puts "[inject-ios-vpn] injected #{EXT_NAME} (#{app_bundle_id}.#{EXT_NAME}) into #{File.basename(project_path)}"
end

inject
