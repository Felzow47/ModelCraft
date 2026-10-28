#!/usr/bin/env node

import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';

const DISALLOWED_DASHES = [
  '\u2014',
  '\u2013',
  '\u2011',
  '\u2012',
  '\u2015',
  '\u2212',
  '\u2010',
  '\uFE58',
  '\uFE63',
  '\uFF0D',
];
const DASH_REGEX = /[\u2014\u2013\u2011\u2012\u2015\u2212\u2010\uFE58\uFE63\uFF0D]/u;

/** Repertoires ignores lors du parcours (nom de dossier, toute profondeur). */
const SKIP_DIR_NAMES = new Set([
  'node_modules',
  '.git',
  'dist',
  '.next',
  'build',
  '.gradle',
  '.cxx',
  'release',
  'out',
  'coverage',
  '.nyc_output',
  '.cache',
  '.turbo',
  '.idea',
  '.pack-cache',
]);

/** Chemins relatifs de dossiers a ignorer entierement. */
const SKIP_DIR_PATHS = [
  'packages/backend/data',
  // Profil Firefox de test de l'extension (web-ext) : fichiers du navigateur, verrouillés quand il tourne.
  'extension/.firefox-profile',
];

/** Extensions binaires - pas de scan texte. */
const BINARY_EXTENSIONS = new Set([
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'ico', 'icns', 'apk', 'aab', 'exe', 'dll',
  'so', 'dylib', 'jar', 'class', 'zip', 'gz', '7z', 'rar', 'pdf', 'woff', 'woff2',
  'ttf', 'otf', 'eot', 'mp3', 'mp4', 'wav', 'webm', 'sqlite', 'db', 'bin', 'dat',
  'keystore', 'jks', 'p12', 'pem', 'crt', 'der', 'lockb', 'map',
]);

function normalizePath(path) {
  return path.split(/[/\\]/).join('/');
}

function shouldSkipDirectory(relativeDirPath, dirName) {
  if (SKIP_DIR_NAMES.has(dirName)) return true;
  const normalized = normalizePath(relativeDirPath);
  return SKIP_DIR_PATHS.some(
    (skipPath) => normalized === skipPath || normalized.startsWith(`${skipPath}/`),
  );
}

function shouldSkipFile(relativeFilePath) {
  const normalized = normalizePath(relativeFilePath);
  const baseName = normalized.split('/').pop() ?? normalized;

  if (baseName === '.DS_Store' || baseName === 'Thumbs.db' || baseName === 'Desktop.ini') {
    return true;
  }

  if (baseName.startsWith('.env') && baseName !== '.env.example') {
    return true;
  }

  const dot = baseName.lastIndexOf('.');
  if (dot === -1) return false;
  const ext = baseName.slice(dot + 1).toLowerCase();
  return BINARY_EXTENSIONS.has(ext);
}

function listProjectFiles(rootDir) {
  const files = [];

  function walk(currentDir) {
    let entries;
    try {
      entries = readdirSync(currentDir, { withFileTypes: true });
    } catch {
      return;
    }

    for (const entry of entries) {
      const absolutePath = join(currentDir, entry.name);
      const relativePath = normalizePath(relative(rootDir, absolutePath));

      if (entry.isDirectory()) {
        if (shouldSkipDirectory(relativePath, entry.name)) continue;
        walk(absolutePath);
        continue;
      }

      if (!entry.isFile()) continue;
      if (shouldSkipFile(relativePath)) continue;
      files.push(relativePath);
    }
  }

  walk(rootDir);
  files.sort();
  return files;
}

function isLikelyBinary(buffer) {
  return buffer.includes(0);
}

function findDashViolations(content) {
  const lines = content.split(/\r?\n/);
  const violations = [];

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (!DASH_REGEX.test(line)) continue;

    const found = [...line].filter((char) => DISALLOWED_DASHES.includes(char));
    const uniqueFound = [...new Set(found)];
    violations.push({
      line: i + 1,
      chars: uniqueFound,
      preview: line.trim().slice(0, 180),
    });
  }

  return violations;
}

function fixDashes(content) {
  let replacementCount = 0;
  const fixed = content.replace(
    /[\u2014\u2013\u2011\u2012\u2015\u2212\u2010\uFE58\uFE63\uFF0D]/gu,
    () => {
      replacementCount += 1;
      return '-';
    },
  );
  return { fixed, replacementCount };
}

function reportCheckViolations(allViolations) {
  console.error('ERROR: disallowed dash characters found. Use "-" only.');

  for (const entry of allViolations) {
    for (const violation of entry.violations) {
      console.error(
        `- ${entry.file}:${violation.line} [${violation.chars.join(' ')}] ${violation.preview}`,
      );
    }
  }
}

function main() {
  const checkOnly = process.argv.includes('--check');
  const root = process.cwd();
  const files = listProjectFiles(root);
  const allViolations = [];
  const fixedFiles = [];

  for (const relativePath of files) {
    const absolutePath = join(root, relativePath);
    let buffer;
    try {
      buffer = readFileSync(absolutePath);
    } catch (error) {
      if (error && typeof error === 'object' && 'code' in error && error.code === 'ENOENT') {
        continue;
      }
      throw error;
    }
    if (isLikelyBinary(buffer)) continue;

    const content = buffer.toString('utf8');
    const violations = findDashViolations(content);
    if (violations.length === 0) continue;

    if (checkOnly) {
      allViolations.push({
        file: relativePath,
        violations,
      });
      continue;
    }

    const { fixed, replacementCount } = fixDashes(content);
    writeFileSync(absolutePath, fixed, 'utf8');
    fixedFiles.push({
      file: relativePath,
      lines: violations.length,
      replacements: replacementCount,
    });
  }

  if (checkOnly) {
    if (allViolations.length === 0) {
      console.log('OK: no disallowed dash characters found.');
      process.exit(0);
    }

    reportCheckViolations(allViolations);
    process.exit(1);
  }

  if (fixedFiles.length === 0) {
    console.log('OK: no disallowed dash characters found.');
    process.exit(0);
  }

  console.log(`Fixed disallowed dash characters in ${fixedFiles.length} file(s):`);
  for (const entry of fixedFiles) {
    const lineLabel = entry.lines === 1 ? 'line' : 'lines';
    console.log(`- ${entry.file} (${entry.lines} ${lineLabel}, ${entry.replacements} replacement(s))`);
  }

  process.exit(0);
}

main();
