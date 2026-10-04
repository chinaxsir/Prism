#!/usr/bin/env node
/**
 * 下载 sing-box 内核并放置为 Tauri sidecar（src-tauri/binaries/，已 gitignore）。
 * 打包后随安装包分发，终端用户无需联网下载内核。
 *
 * 用法: node scripts/fetch-kernel.cjs [--force]
 */

const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const KERNEL_VERSION = "1.11.3";
const ROOT = path.join(__dirname, "..");
const OUT_DIR = path.join(ROOT, "src-tauri", "binaries");

/** 当前平台 → [Rust target triplet, 发行包文件名]；
 *  也可用 --triplet <rust-triplet> 强制指定（CI 交叉构建场景） */
const TRIPLET_MAP = {
  "x86_64-pc-windows-msvc": `sing-box-${KERNEL_VERSION}-windows-amd64.zip`,
  "aarch64-pc-windows-msvc": `sing-box-${KERNEL_VERSION}-windows-arm64.zip`,
  "x86_64-apple-darwin": `sing-box-${KERNEL_VERSION}-darwin-amd64.tar.gz`,
  "aarch64-apple-darwin": `sing-box-${KERNEL_VERSION}-darwin-arm64.tar.gz`,
  "x86_64-unknown-linux-gnu": `sing-box-${KERNEL_VERSION}-linux-amd64.tar.gz`,
  "aarch64-unknown-linux-gnu": `sing-box-${KERNEL_VERSION}-linux-arm64.tar.gz`,
};

function targetInfo() {
  const forcedIdx = process.argv.indexOf("--triplet");
  if (forcedIdx !== -1 && process.argv[forcedIdx + 1]) {
    const triplet = process.argv[forcedIdx + 1];
    const asset = TRIPLET_MAP[triplet];
    if (!asset) throw new Error(`--triplet 不支持: ${triplet}`);
    return [triplet, asset];
  }
  const key = `${process.platform}-${process.arch}`;
  const map = {
    "win32-x64": "x86_64-pc-windows-msvc",
    "win32-arm64": "aarch64-pc-windows-msvc",
    "darwin-x64": "x86_64-apple-darwin",
    "darwin-arm64": "aarch64-apple-darwin",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
  };
  const triplet = map[key];
  if (!triplet) {
    throw new Error(`不支持的平台: ${key}，请手动下载 sing-box 放入 src-tauri/binaries/`);
  }
  return [triplet, TRIPLET_MAP[triplet]];
}

/** 下载源：官方 + GitHub 镜像（与 Rust 端 kernel_download.rs 保持一致） */
function mirrorUrls(official) {
  const urls = [official];
  if (process.env.PRISM_KERNEL_MIRROR) {
    urls.push(`${process.env.PRISM_KERNEL_MIRROR}${official}`);
  }
  urls.push(
    `https://ghfast.top/${official}`,
    `https://gh-proxy.com/${official}`,
    `https://mirror.ghproxy.com/${official}`,
  );
  return urls;
}

function candidateUrls(asset) {
  return mirrorUrls(`https://github.com/SagerNet/sing-box/releases/download/v${KERNEL_VERSION}/${asset}`);
}

/** geoip/geosite 数据库（平台无关）：legacy GEOIP/GEOSITE 规则需要本地库，
 *  缺失时内核会在启动期尝试联网下载（走代理出站，必失败），故随包分发 */
const GEO_DBS = [
  ["geoip.db", "https://github.com/SagerNet/sing-geoip/releases/latest/download/geoip.db"],
  ["geosite.db", "https://github.com/SagerNet/sing-geosite/releases/latest/download/geosite.db"],
];

async function ensureGeoDbs(force) {
  for (const [name, official] of GEO_DBS) {
    const dest = path.join(OUT_DIR, name);
    if (!force && fs.existsSync(dest)) {
      console.log(`[fetch-kernel] 已存在，跳过: src-tauri/binaries/${name}`);
      continue;
    }
    await download(mirrorUrls(official), dest);
    console.log(`[fetch-kernel] 数据库就绪: src-tauri/binaries/${name}`);
  }
}

async function download(urls, dest) {
  let lastErr;
  for (const url of urls) {
    try {
      console.log(`[fetch-kernel] downloading: ${url}`);
      const resp = await fetch(url, { redirect: "follow" });
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const buf = Buffer.from(await resp.arrayBuffer());
      if (buf.length < 1024 * 1024) throw new Error(`文件过小 (${buf.length} B)，疑似损坏`);
      fs.writeFileSync(dest, buf);
      return;
    } catch (e) {
      console.warn(`[fetch-kernel] 源失败: ${e.message}`);
      lastErr = e;
    }
  }
  throw lastErr ?? new Error("没有可用的下载源");
}

/** 解压 zip/tar.gz：优先系统 tar（Windows 10+ 自带 bsdtar 支持 zip） */
function extract(archive, destDir) {
  try {
    execFileSync("tar", ["-xf", archive, "-C", destDir], { stdio: "inherit" });
    return;
  } catch (e) {
    if (process.platform === "win32" && archive.endsWith(".zip")) {
      execFileSync(
        "powershell",
        ["-NoProfile", "-NonInteractive", "-Command",
          "Expand-Archive -LiteralPath $args[0] -DestinationPath $args[1] -Force", "--", archive, destDir],
        { stdio: "inherit" },
      );
      return;
    }
    throw e;
  }
}

function findBinary(dir, exeSuffix) {
  const want = `sing-box${exeSuffix}`;
  const stack = [dir];
  while (stack.length) {
    const cur = stack.pop();
    for (const entry of fs.readdirSync(cur, { withFileTypes: true })) {
      const p = path.join(cur, entry.name);
      if (entry.isDirectory()) stack.push(p);
      else if (entry.name === want) return p;
    }
  }
  throw new Error(`压缩包内未找到 ${want}`);
}

async function main() {
  const force = process.argv.includes("--force");
  const [triplet, asset] = targetInfo();
  const exeSuffix = triplet.includes("windows") ? ".exe" : "";
  const target = path.join(OUT_DIR, `sing-box-${triplet}${exeSuffix}`);

  fs.mkdirSync(OUT_DIR, { recursive: true });

  // 数据库与内核独立获取：内核已存在时也要保证数据库就位
  await ensureGeoDbs(force);

  if (!force && fs.existsSync(target)) {
    console.log(`[fetch-kernel] 已存在，跳过: ${path.relative(ROOT, target)}（--force 可重新下载）`);
    return;
  }

  const stageDir = path.join(OUT_DIR, `stage-${Date.now()}`);
  fs.mkdirSync(stageDir, { recursive: true });

  try {
    const archive = path.join(stageDir, asset);
    await download(candidateUrls(asset), archive);
    console.log("[fetch-kernel] extracting…");
    extract(archive, stageDir);
    const found = findBinary(stageDir, exeSuffix);
    fs.copyFileSync(found, target);
    if (process.platform !== "win32") fs.chmodSync(target, 0o755);
    console.log(`[fetch-kernel] sidecar 就绪: ${path.relative(ROOT, target)}`);
  } finally {
    fs.rmSync(stageDir, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error(`[fetch-kernel] 失败: ${e.message}`);
  process.exit(1);
});
