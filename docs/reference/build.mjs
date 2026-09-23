// Generates docs/reference/data.js for the teaching site.
//
//   node docs/reference/build.mjs [--bin <path>] [--check]
//
// The Clap grammar (`monica commands --json`) is the only source of truth for
// command names, flags and required fields; examples.json only adds prose,
// sample lines and captured output. Every sample line is re-validated against
// that grammar here — through docs/reference/grammar.js, the same checker the
// practice mode runs in the browser — so a renamed flag fails the build
// instead of teaching a command that no longer exists.
//
// --check compares the existing data.js against the live grammar and exits
// non-zero on drift without rewriting anything.

import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const { create: createGrammar, gradeDrill } = require('./grammar.js');

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, '..', '..');
const OUT = join(HERE, 'data.js');
const EXAMPLES = join(HERE, 'examples.json');
const CLI_NAME = 'monica';

const args = process.argv.slice(2);
const checkOnly = args.includes('--check');
const binFlag = args.indexOf('--bin');
const candidateBin = binFlag >= 0 ? args[binFlag + 1] : null;

function findBinary() {
  const names = candidateBin
    ? [candidateBin]
    : [
        join(REPO, 'target', 'release', 'monica-pass.exe'),
        join(REPO, 'target', 'release', 'monica-pass'),
        join(REPO, 'target', 'debug', 'monica-pass.exe'),
        join(REPO, 'target', 'debug', 'monica-pass'),
      ];
  for (const name of names) {
    if (existsSync(name)) return name;
  }
  throw new Error('no monica-pass binary found; build it first or pass --bin <path>');
}

function readGrammar(binary) {
  // Discovery never touches the vault, but keep it pointed at an empty home so
  // a stray config read could never reach a real one.
  const sandbox = mkdtempSync(join(tmpdir(), 'monica-teach-build-'));
  const stdout = execFileSync(binary, ['commands', '--json'], {
    cwd: sandbox,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    env: {
      ...process.env,
      HOME: sandbox,
      USERPROFILE: sandbox,
      LOCALAPPDATA: sandbox,
      XDG_STATE_HOME: sandbox,
    },
  });
  return JSON.parse(stdout).data;
}

const GLOBAL_IDS = new Set();
const globals = [];

function flatten(node, out) {
  const own = [];
  for (const arg of node.arguments ?? []) {
    if (arg.global) {
      if (!GLOBAL_IDS.has(arg.id)) {
        GLOBAL_IDS.add(arg.id);
        globals.push(publicArg(arg));
      }
      continue;
    }
    own.push(publicArg(arg));
  }
  const record = {
    key: node.path.join(' '),
    path: node.path,
    name: node.name,
    parent: node.path.slice(0, -1).join(' '),
    summary: node.summary ?? '',
    aliases: node.aliases ?? [],
    args: own,
    jsonSupported: node.json_supported === true,
    longRunning: node.long_running === true,
    subcommandRequired: node.subcommand_required === true,
    argGroups: (node.argument_groups ?? []).map((group) => ({
      oneOf: group.required_one_of ?? [],
      multiple: group.multiple === true,
    })),
    secretRequired: node.secret_input?.required ?? [],
    secretFlag: node.secret_input?.flag ?? '--secrets-stdin',
  };
  if (node.path.length) out.push(record);
  for (const child of node.commands ?? []) flatten(child, out);
}

function publicArg(arg) {
  return {
    id: arg.id,
    long: arg.long,
    short: arg.short,
    aliases: arg.aliases ?? [],
    positional: arg.positional === true,
    required: arg.required === true,
    repeatable: arg.repeatable === true,
    takesValue: arg.takes_value !== false,
    valueNames: arg.value_names ?? [],
    choices: arg.choices ?? [],
    choiceAliases: arg.choice_aliases ?? [],
    ignoreCase: arg.ignore_case === true,
    defaults: arg.defaults ?? [],
    help: arg.help ?? '',
  }
}

// --- checks ----------------------------------------------------------------

const errors = [];
const warnings = [];

function fail(exampleKey, message) {
  errors.push(`${exampleKey}: ${message}`);
}

function checkCapturedOutput(key, example) {
  if (example.tested === true) {
    if (typeof example.out !== 'string' || !example.out.trim()) {
      fail(key, `marked tested but carries no captured output: ${example.cmd}`);
    }
    return;
  }
  if (example.tested !== false) {
    fail(key, `example needs tested: true or false: ${example.cmd}`);
    return;
  }
  if (!example.reason) {
    fail(key, `tested: false needs a reason the output was not captured`);
  }
}

// --- main ------------------------------------------------------------------

const binary = findBinary();
const rawGrammar = readGrammar(binary);
const nodes = [];
flatten(rawGrammar, nodes);
const grammarKeys = new Set(nodes.map((node) => node.key));

const authored = JSON.parse(readFileSync(EXAMPLES, 'utf8'));
const authoredKeys = new Set(Object.keys(authored.commands));

// A teaching entry may sit outside `commands --json` on purpose — today only
// `keys export`, the human-only exit path. Such an entry carries its own args
// so the practice mode can grade it like any other command.
const handAuthored = [];
const handNodes = [];
for (const [key, entry] of Object.entries(authored.commands)) {
  if (grammarKeys.has(key) || !entry.handAuthored) continue;
  handAuthored.push(key);
  if (Array.isArray(entry.args) && entry.args.length) {
    handNodes.push({
      key,
      path: key.split(' '),
      name: key.split(' ').at(-1),
      parent: key.split(' ').slice(0, -1).join(' '),
      summary: entry.summary ?? '',
      aliases: [],
      args: entry.args,
      jsonSupported: entry.jsonSupported === true,
      longRunning: entry.longRunning === true,
      secretRequired: entry.secretRequired ?? [],
      secretFlag: '--secrets-stdin',
    });
    warnings.push(`${key}: validated against hand-authored args, not the live grammar`);
  } else {
    warnings.push(`${key}: no args, so its flags are not validated`);
  }
}

const engine = createGrammar({ cli: CLI_NAME, commands: [...nodes, ...handNodes], globals });
const byKey = engine.byKey;

function checkSample(key, example) {
  if (typeof example.cmd !== 'string' || !example.cmd.trim()) {
    fail(key, 'example has no cmd');
    return;
  }
  if (!handAuthored.includes(key) && !grammarKeys.has(key) && !byKey.has(key)) return;
  const result = engine.validate(example.cmd, { requirePrefix: true });
  // Some entries teach a mistake on purpose — `monica webdav` shows what the
  // group does when it gets no subcommand. Such a line must be rejected by the
  // grammar, otherwise the captured error output is a lie.
  if (example.teachesError === true) {
    if (result.ok) fail(key, `marked teachesError but the grammar accepts it: ${example.cmd}`);
  } else {
    for (const issue of result.errors) {
      fail(key, `${issue.code} — ${issue.message} | ${example.cmd}`);
    }
  }
  if (result.usedSecretFlag && !example.secrets) {
    fail(key, `uses --secrets-stdin but shows no stdin payload: ${example.cmd}`);
  }
}

for (const node of nodes) {
  if (!authoredKeys.has(node.key)) {
    errors.push(`grammar command \`${node.key}\` has no teaching entry`);
  }
}

const versionMatch = (() => {
  try {
    return execFileSync(binary, ['--version'], { encoding: 'utf8' }).trim();
  } catch {
    return '';
  }
})();

for (const [key, entry] of Object.entries(authored.commands)) {
  const node = byKey.get(key);
  if (!grammarKeys.has(key) && !entry.handAuthored) {
    errors.push(
      `teaching entry \`${key}\` is not in the grammar (mark it handAuthored if that is deliberate)`,
    );
    continue;
  }
  if (!entry.summaryZh) errors.push(`\`${key}\` has no Chinese summary`);
  if (!entry.group) errors.push(`\`${key}\` has no group`);
  if (!Array.isArray(entry.examples) || entry.examples.length === 0) {
    errors.push(`\`${key}\` has no examples`);
    continue;
  }
  let sawSecretShape = false;
  for (const example of entry.examples) {
    checkSample(key, example);
    checkCapturedOutput(key, example);
    if (example.secrets) sawSecretShape = true;
  }
  if ((node?.secretRequired ?? []).length && !sawSecretShape) {
    errors.push(
      `\`${key}\` needs secret fields (${node.secretRequired.join(', ')}) but no example shows them`,
    );
  }
}

const usedGroups = new Set();
for (const entry of Object.values(authored.commands)) {
  if (entry.group) usedGroups.add(entry.group);
}
for (const group of authored.groups ?? []) {
  if (!usedGroups.has(group.id)) {
    warnings.push(`group "${group.id}" has no commands`);
  }
}
usedGroups.forEach((id) => {
  if (!(authored.groups ?? []).some((group) => group.id === id)) {
    errors.push(`command uses unknown group "${id}"`);
  }
});

for (const drill of authored.drills ?? []) {
  if (!drill.expect?.key) {
    errors.push(`drill "${drill.id}" has no expect.key`);
    continue;
  }
  if (!byKey.has(drill.expect.key)) {
    errors.push(`drill "${drill.id}" points at unknown command \`${drill.expect.key}\``);
  }
  if (!drill.answer || !drill.why) {
    errors.push(`drill "${drill.id}" needs an answer and a why`);
    continue;
  }
  const answer = gradeDrill(engine, drill.answer, drill);
  for (const issue of answer.result.errors) {
    fail(`drill ${drill.id}`, `${issue.code} — ${issue.message} | ${drill.answer}`);
  }
  if (!answer.ok) {
    fail(`drill ${drill.id}`, `answer does not satisfy its own expect: ${answer.problems.join(' / ')} | ${drill.answer}`);
  }
  const holder = { cmd: drill.answer, tested: true, out: '.' };
  checkCapturedOutput(drill.id, holder);
}

if (errors.length) {
  console.error(`build.mjs found ${errors.length} problem(s):`);
  for (const line of errors) console.error(`  - ${line}`);
  process.exit(1);
}

const commands = [];
for (const [key, entry] of Object.entries(authored.commands)) {
  const node = byKey.get(key);
  commands.push({
    key,
    path: node?.path ?? key.split(' '),
    name: node?.name ?? key.split(' ').at(-1),
    parent: node?.parent ?? (key.includes(' ') ? key.split(' ').slice(0, -1).join(' ') : ''),
    summary: node?.summary ?? entry.summary ?? '',
    aliases: node?.aliases ?? [],
    args: node?.args ?? [],
    jsonSupported: node?.jsonSupported ?? false,
    longRunning: node?.longRunning ?? false,
    subcommandRequired: node?.subcommandRequired === true,
    argGroups: node?.argGroups ?? [],
    secretRequired: node?.secretRequired ?? [],
    group: entry.group,
    summaryZh: entry.summaryZh,
    whenToUse: entry.whenToUse ?? '',
    handAuthored: !grammarKeys.has(key),
    pitfalls: entry.pitfalls ?? [],
    examples: entry.examples,
  });
}

const data = {
  meta: {
    cli: CLI_NAME,
    version: versionMatch,
    grammarVersion: rawGrammar.schema_version ?? null,
    generatedFrom: 'monica commands --json',
    commandCount: nodes.length,
    groups: authored.groups ?? [],
    globals,
  },
  commands,
  drills: authored.drills ?? [],
  notes: authored.notes ?? {},
};

const banner =
  'Generated by docs/reference/build.mjs — do not edit by hand.\n' +
  'Grammar: `monica commands --json`. Prose and samples: docs/reference/examples.json.\n';

const header = banner
  .trimEnd()
  .split('\n')
  .map((line) => `// ${line}`)
  .join('\n');

if (checkOnly) {
  const existing = readFileSync(OUT, 'utf8');
  const payload = existing.slice(existing.indexOf('{'), existing.lastIndexOf('}') + 1);
  const shape = (list) =>
    JSON.stringify(list.map((c) => [c.key, c.args.map((a) => a.long ?? a.id).sort()]));
  const before = shape(JSON.parse(payload).commands);
  const after = shape(commands);
  if (before !== after) {
    console.error('data.js is stale: the command grammar changed, re-run build.mjs');
    process.exit(1);
  }
  console.log(`data.js matches the live grammar (${nodes.length} commands)`);
  process.exit(0);
}

writeFileSync(
  OUT,
  `${header}\nwindow.MONICA_TEACH = ${JSON.stringify({ ...data, generatedAt: new Date().toISOString().slice(0, 10) }, null, 1)};\n`,
  'utf8',
);

for (const line of warnings) console.warn(`warning: ${line}`);
if (handAuthored.length) {
  console.warn(`hand-authored (outside commands --json): ${handAuthored.join(', ')}`);
}
console.log(
  `wrote ${OUT.replace(/\\/g, '/')} — ${commands.length} entries, ${nodes.length} grammar commands, ${versionMatch || 'version unknown'}`,
);
