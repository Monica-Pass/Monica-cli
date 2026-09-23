// Shared command-line validator for the teaching site.
//
// One implementation, two callers: docs/reference/build.mjs uses it to reject
// sample lines that the real Clap grammar would not accept, and the browser
// practice mode uses it to grade what the reader types. It never runs anything;
// it only checks a line against the grammar recorded in data.js.
//
// The point is to agree with the real parser on every shorthand a person can
// type: command and flag aliases, attached values (`-cwork`), clustered shorts
// (`-ws`), `--flag=value`, global options anywhere in the line including before
// the command, `--` as end-of-options, and the help short-circuit (`-h`/`--help`
// anywhere, the built-in `help` word, root `--version`). Where the two disagree,
// the binary is right; docs/reference/build.mjs is run against it, and the drills
// and sample lines are re-checked on every build.
//
// Loaded as a classic <script> (window.MONICA_GRAMMAR) and as CommonJS by
// build.mjs, so file:// pages work without a bundler.

(function (root, factory) {
  const api = factory();
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  root.MONICA_GRAMMAR = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, function () {
  'use strict';

  const PREFIXES = ['monica', 'monica-pass', 'monica-pass.exe'];
  // Only the root carries `--version`/`-V`. These print and exit before any
  // further argument is looked at; the built-in `help` word is handled by the
  // command walk, because clap only gives it to commands that have subcommands.
  const ROOT_HELP = ['--version', '-V', '--help', '-h'];
  const HELP_NOTE = '这一行只是在打印帮助，不会执行任何操作。';

  function tokenize(line) {
    const tokens = [];
    let current = '';
    let quote = null;
    let started = false;
    for (const ch of line) {
      if (quote) {
        if (ch === quote) quote = null;
        else current += ch;
        started = true;
        continue;
      }
      if (ch === '"' || ch === "'") {
        quote = ch;
        started = true;
        continue;
      }
      if (/\s/.test(ch)) {
        if (started) tokens.push(current);
        current = '';
        started = false;
        continue;
      }
      current += ch;
      started = true;
    }
    if (quote) throw new Error('unclosed quote');
    if (started) tokens.push(current);
    return tokens;
  }

  function flagLabel(arg) {
    if (arg.long) return '--' + arg.long;
    if (arg.short) return '-' + arg.short;
    return arg.id;
  }

  // Clap matches a choice against the canonical name *and* its aliases, and only
  // folds case when the argument opts into `ignore_case` — `language zh-cn` and
  // `language zh` are accepted, `--provider GITHUB` is not.
  function matchesChoice(arg, value) {
    const names = (arg.choices || []).concat(arg.choiceAliases || []);
    if (!names.length) return true;
    if (arg.ignoreCase === true) {
      const lowered = String(value).toLowerCase();
      return names.some(function (name) {
        return name.toLowerCase() === lowered;
      });
    }
    return names.indexOf(value) >= 0;
  }

  function choicesText(arg) {
    const names = (arg.choices || []).join(' / ');
    const aliases = arg.choiceAliases || [];
    return aliases.length ? names + '（也可写 ' + aliases.join(' / ') + '）' : names;
  }

  function argMaps(node, globals) {
    const longs = new Map();
    const shorts = new Map();
    for (const arg of node.args) {
      if (arg.long) longs.set('--' + arg.long, arg);
      for (const alias of arg.aliases || []) longs.set('--' + alias, arg);
      if (arg.short && !arg.positional) shorts.set('-' + arg.short, arg);
    }
    for (const arg of globals) {
      if (arg.long && !longs.has('--' + arg.long)) longs.set('--' + arg.long, arg);
      for (const alias of arg.aliases || []) {
        if (!longs.has('--' + alias)) longs.set('--' + alias, arg);
      }
      if (arg.short && !shorts.has('-' + arg.short)) shorts.set('-' + arg.short, arg);
    }
    return { longs, shorts };
  }

  function create(rules) {
    const cli = rules.cli || 'monica';
    const globals = rules.globals || [];
    const globalLongs = new Map();
    const globalShorts = new Map();
    for (const arg of globals) {
      if (arg.long) globalLongs.set('--' + arg.long, arg);
      for (const alias of arg.aliases || []) globalLongs.set('--' + alias, arg);
      if (arg.short) globalShorts.set('-' + arg.short, arg);
    }

    // Clap's global flags (`--json`, `--lang en`, `-C file`) are legal anywhere
    // in the line, including before the command name, so the command walk has
    // to step over them the way the real parser does. Returns the flag, the
    // value it swallowed and the index the next word starts at.
    function globalAt(tokens, index) {
      const token = tokens[index];
      if (!token || token.length < 2 || token[0] !== '-' || token === '--') return null;
      if (token.startsWith('--')) {
        const name = token.split('=')[0];
        const arg = globalLongs.get(name);
        if (!arg) return null;
        const inline = arg.takesValue && token.includes('=');
        if (!arg.takesValue) return { arg, raw: name, next: index + 1 };
        return {
          arg,
          raw: name,
          value: inline ? token.slice(token.indexOf('=') + 1) : tokens[index + 1],
          next: inline ? index + 1 : index + 2,
        };
      }
      const name = token.slice(0, 2);
      const arg = globalShorts.get(name);
      if (!arg) return null;
      const attached = arg.takesValue && token.length > 2;
      if (!arg.takesValue) return { arg, raw: name, next: index + 1 };
      return {
        arg,
        raw: name,
        value: attached ? token.slice(2) : tokens[index + 1],
        next: attached ? index + 1 : index + 2,
      };
    }

    const byKey = new Map();
    for (const node of rules.commands) byKey.set(node.key, node);
    // One level of the command tree at a time, aliases included, so `ls`,
    // `dav status` and `keys gpg` resolve the way the real binary does.
    const children = new Map();
    function addBranch(parent, token, key) {
      let map = children.get(parent);
      if (!map) {
        map = new Map();
        children.set(parent, map);
      }
      if (!map.has(token)) map.set(token, key);
    }
    for (const node of rules.commands) {
      const parent = node.parent || '';
      const segments = node.path && node.path.length ? node.path : node.key.split(' ');
      addBranch(parent, segments[segments.length - 1], node.key);
      for (const alias of node.aliases || []) addBranch(parent, alias, node.key);
    }

    // Bare names of a group's subcommands, in grammar order, so an error
    // message can say what clap would have accepted.
    function childNames(key) {
      const names = [];
      for (const node of rules.commands) {
        if (node.parent === key) names.push(node.path ? node.path[node.path.length - 1] : node.key.split(' ').pop());
      }
      return names;
    }

    // Walks the command tree the way clap does, stepping over global flags.
    // `given` are the globals seen on the way, which still have to count as
    // flags the reader typed; `rest` is the indices left to parse as arguments.
    function resolve(tokens) {
      let parent = '';
      let node = null;
      const depth = new Set();
      const claimed = new Set();
      const given = [];
      let firstCommandAt = -1;
      let helpWordAt = -1;
      let index = 0;
      while (index < tokens.length && depth.size < 4) {
        if (helpWordAt < 0) {
          const skipped = globalAt(tokens, index);
          if (skipped) {
            for (let i = index; i < Math.min(skipped.next, tokens.length); i += 1) claimed.add(i);
            given.push({ arg: skipped.arg, raw: skipped.raw, value: skipped.value, at: index });
            index = skipped.next;
            continue;
          }
        }
        const map = children.get(parent);
        if (tokens[index] === 'help' && helpWordAt < 0 && map && map.size) {
          // `keys help` and `webdav help login` print help because clap gives every
          // command with children that word; `list help` is a stray positional
          // because `list` has none. In help mode only command names may follow.
          helpWordAt = index;
          claimed.add(index);
          // The help word is a subcommand, so globals written before it are
          // still deferred past the point where help prints.
          if (firstCommandAt < 0) firstCommandAt = index;
          index += 1;
          continue;
        }
        const next = map && map.get(tokens[index]);
        if (!next) break;
        node = byKey.get(next);
        parent = next;
        depth.add(index);
        claimed.add(index);
        if (firstCommandAt < 0) firstCommandAt = index;
        index += 1;
      }
      const rest = [];
      for (let i = 0; i < tokens.length; i += 1) {
        if (!claimed.has(i)) rest.push(i);
      }
      return { node, given, rest, firstCommandAt, helpWordAt };
    }

    // validate(line) -> { ok, errors: [{code, message}], node, command }
    function validate(line, options) {
      const opts = options || {};
      const errors = [];
      const push = (code, message) => errors.push({ code, message });
      let tokens;
      try {
        tokens = tokenize(line);
      } catch (error) {
        return { ok: false, errors: [{ code: 'unclosed_quote', message: '引号没有闭合。' }], node: null };
      }
      if (!tokens.length) {
        return {
          ok: false,
          errors: [{ code: 'empty', message: '还没有输入命令。' }],
          node: null,
        };
      }
      if (PREFIXES.includes(tokens[0])) {
        tokens = tokens.slice(1);
      } else if (opts.requirePrefix) {
        return {
          ok: false,
          errors: [
            {
              code: 'missing_prefix',
              message: '示例行以 `' + cli + '` 开头，后面才是子命令。',
            },
          ],
          node: null,
        };
      }
      if (!tokens.length) {
        return {
          ok: true,
          errors: [{ code: 'root_help', message: '不带子命令时会打开终端管理器（TUI）。' }],
          node: null,
        };
      }
      // `--json help` and `help` reach the same place, so the first word that is
      // not a global flag is the one that decides.
      let head = 0;
      let headBadChoice = null;
      for (;;) {
        const skipped = globalAt(tokens, head);
        if (!skipped) break;
        if (
          skipped.value !== undefined &&
          !headBadChoice &&
          !matchesChoice(skipped.arg, skipped.value)
        ) {
          headBadChoice =
            '`' + skipped.raw + '` 只能取 ' + choicesText(skipped.arg) + '，不是 `' + skipped.value + '`。';
        }
        head = skipped.next;
      }
      if (head >= tokens.length || ROOT_HELP.includes(tokens[head])) {
        // Nothing took over as a subcommand, so clap validates the root-level
        // globals it already read before it looks at the help flag after them.
        if (headBadChoice) {
          return { ok: false, errors: [{ code: 'bad_choice', message: headBadChoice }], node: null };
        }
      }
      if (head >= tokens.length) {
        return {
          ok: true,
          errors: [{ code: 'root_help', message: '只带了全局选项，没有子命令，会打开终端管理器（TUI）。' }],
          node: null,
        };
      }
      if (tokens[head] === '--version' || tokens[head] === '-V') {
        return {
          ok: true,
          errors: [{ code: 'root_version', message: '这只打印 CLI 版本，不碰任何数据。' }],
          node: null,
        };
      }
      if (ROOT_HELP.includes(tokens[head])) {
        return {
          ok: true,
          errors: [
            {
              code: 'root_help',
              message: '这是查看帮助，不是执行操作；`monica commands` 才能列出全部命令。',
            },
          ],
          node: null,
        };
      }
      const found = resolve(tokens);
      if (found.helpWordAt >= 0) {
        if (found.rest.length) {
          const from = found.rest[0];
          return {
            ok: false,
            errors: [
              {
                code: 'unknown_command',
                message:
                  '`help` 之后只能接命令名，`' + tokens.slice(from, from + 2).join(' ') + '` 不是。',
              },
            ],
            node: found.node,
          };
        }
        return {
          ok: true,
          errors: [{ code: 'help_shortcut', message: HELP_NOTE }],
          node: found.node,
          command: found.node ? found.node.key : '',
        };
      }
      if (!found.node) {
        const from = found.rest[0];
        return {
          ok: false,
          errors: [
            {
              code: 'unknown_command',
              message:
                '没有这个命令：`' + tokens.slice(from, from + 2).join(' ') + '`。用 `monica commands` 查全部命令。',
            },
          ],
          node: null,
        };
      }
      const node = found.node;
      const restAt = found.rest;
      const rest = restAt.map(function (index) {
        return tokens[index];
      });
      const { longs, shorts } = argMaps(node, globals);

      // Where clap would stop and print help: an exact `--help`, or a cluster that
      // reaches `h` while it is still reading flags. A bare `--` ends the search,
      // because after it `-h` is ordinary data.
      let helpAt = null;
      for (const at of restAt) {
        const token = tokens[at];
        if (token === '--') break;
        if (token === '--help') {
          helpAt = at;
          break;
        }
        if (token.startsWith('--') || token.length < 2 || token[0] !== '-') continue;
        for (let pos = 1; pos < token.length; pos += 1) {
          const arg = shorts.get('-' + token[pos]);
          if (!arg) break;
          if (arg.id === 'help') {
            helpAt = at;
            break;
          }
          if (arg.takesValue) break;
        }
        if (helpAt !== null) break;
      }

      // `webdav` is a group: clap needs the subcommand, and a word that is not
      // one of them is a wrong subcommand rather than a stray positional.
      const groupNeedsChild = node.subcommandRequired === true;
      if (groupNeedsChild && helpAt === null) {
        const names = childNames(node.key);
        const typed = rest.filter(function (token) {
          return !token.startsWith('-');
        });
        push(
          'missing_subcommand',
          typed.length
            ? '`' + node.key + ' ' + typed[0] + '` 不是已知子命令，`' + node.key + '` 可用：' + names.join(' / ') + '。'
            : '`' + node.key + '` 只是分组，还要接一个子命令：' + names.join(' / ') + '。',
        );
      }
      const seen = new Set();
      const given = [];
      const positionals = [];
      let usedSecretFlag = false;
      let usedJsonFlag = false;

      function record(arg, raw, value) {
        given.push({
          id: arg.id,
          long: arg.long ? '--' + arg.long : null,
          short: arg.short ? '-' + arg.short : null,
          raw,
          value: value === undefined ? null : value,
        });
        if (arg.id === 'secrets_stdin') usedSecretFlag = true;
        if (arg.id === 'json') usedJsonFlag = true;
      }

      const globalChoices = [];
      for (const typed of found.given) {
        record(typed.arg, typed.raw, typed.value);
        if (typed.value !== undefined && !matchesChoice(typed.arg, typed.value)) {
          globalChoices.push({
            at: typed.at,
            message:
              '`' + typed.raw + '` 只能取 ' + choicesText(typed.arg) + '，不是 `' + typed.value + '`。',
          });
        }
      }
      for (const issue of globalChoices) {
        if (helpAt !== null) {
          // clap pushes a root-level global down to the subcommand, so the subcommand's
          // help prints before that value is ever checked; one written after the
          // command name is parsed in place and fails first.
          if (found.firstCommandAt >= 0 && issue.at < found.firstCommandAt) continue;
          if (issue.at > helpAt) continue;
        }
        push('bad_choice', issue.message);
      }

      // clap stops reading options at a bare `--`, so everything after it is a
      // value even when it looks like a flag: `monica note work -- --json`.
      let onlyPositional = false;

      for (let i = 0; i < rest.length; i += 1) {
        if (restAt[i] === helpAt) break;
        const token = rest[i];
        if (onlyPositional) {
          positionals.push(token);
          continue;
        }
        if (token === '--') {
          onlyPositional = true;
          continue;
        }
        if (token.startsWith('--')) {
          const parts = token.split('=');
          const name = parts[0];
          const inline = parts.length > 1 ? parts.slice(1).join('=') : undefined;
          const arg = longs.get(name);
          if (!arg) {
            push('unknown_flag', '`' + node.key + '` 没有 `' + name + '` 这个选项。');
            continue;
          }
          seen.add(arg.id);
          if (arg.takesValue) {
            const value = inline !== undefined ? inline : rest[++i];
            if (value === undefined) {
              push('missing_value', '`' + name + '` 还需要一个取值。');
              continue;
            }
            record(arg, name, value);
            if (!matchesChoice(arg, value)) {
              push('bad_choice', '`' + name + '` 只能取 ' + choicesText(arg) + '，不是 `' + value + '`。');
            }
          } else {
            record(arg, name, null);
            if (inline !== undefined) {
              push('unexpected_value', '`' + name + '` 是开关，不带取值。');
            }
          }
          continue;
        }
        if (token.length > 1 && token[0] === '-') {
          // `-ws` is `-w -s`, and the first short that takes a value swallows the
          // rest of the cluster: `-cwork` is `-c work`.
          for (let pos = 1; pos < token.length; pos += 1) {
            const name = '-' + token[pos];
            const arg = shorts.get(name);
            if (!arg) {
              push('unknown_flag', '`' + node.key + '` 没有 `' + name + '` 这个短选项。');
              break;
            }
            seen.add(arg.id);
            if (!arg.takesValue) {
              record(arg, name, null);
              continue;
            }
            const attached = token.slice(pos + 1);
            const value = attached || rest[++i];
            if (value === undefined) {
              push('missing_value', '`' + name + '` 还需要一个取值。');
              break;
            }
            record(arg, name, value);
            if (!matchesChoice(arg, value)) {
              push('bad_choice', '`' + name + '` 只能取 ' + choicesText(arg) + '，不是 `' + value + '`。');
            }
            break;
          }
          continue;
        }
        positionals.push(token);
      }

      if (helpAt !== null) {
        // Whatever came before the help flag parsed, and clap prints from there:
        // `keys gpg n -h` shows help even though the key group is still unsatisfied,
        // while `keys gpg --bogus -h` fails on the unknown option it met first.
        const declaredEarly = node.args.filter(function (arg) {
          return arg.positional;
        });
        positionals.forEach(function (value, index) {
          const arg = declaredEarly[Math.min(index, declaredEarly.length - 1)];
          if (arg && !matchesChoice(arg, value)) {
            push(
              'bad_choice',
              '`' + node.key + '` 的位置参数只能取 ' + choicesText(arg) + '，不是 `' + value + '`。',
            );
          }
        });
        if (errors.length) return { ok: false, errors, node, command: node.key };
        return {
          ok: true,
          errors: [{ code: 'help_shortcut', message: HELP_NOTE }],
          node,
          command: node.key,
          positionals,
          given,
          usedJsonFlag,
          usedSecretFlag,
        };
      }

      const declared = node.args.filter(function (arg) {
        return arg.positional;
      });
      // clap fills positionals in order, so a name counts as supplied once
      // enough values came before it.
      declared.forEach(function (arg, index) {
        if (index < positionals.length) seen.add(arg.id);
      });
      if (groupNeedsChild) {
        // The stray word already got the subcommand message above.
      } else if (declared.length) {
        const requiredCount = declared.filter(function (arg) {
          return arg.required;
        }).length;
        const last = declared[declared.length - 1];
        const max = last.repeatable ? Infinity : declared.length;
        if (positionals.length < requiredCount) {
          push(
            'too_few_positionals',
            '`' + node.key + '` 至少需要 ' + requiredCount + ' 个位置参数（' + valueNames(declared) + '），现在只给了 ' + positionals.length + ' 个。',
          );
        } else if (positionals.length > max) {
          push('too_many_positionals', '`' + node.key + '` 最多接受 ' + max + ' 个位置参数。');
        }
      } else if (positionals.length) {
        push('no_positionals', '`' + node.key + '` 不接受位置参数。');
      }

      // clap validates possible_values on positionals too: `monica language
      // zz-name` is a bad value, not just an unfamiliar name.
      if (!groupNeedsChild) {
        positionals.forEach(function (value, index) {
          if (!declared.length) return;
          const last = declared.length - 1;
          const arg = declared[Math.min(index, last)];
          if (!matchesChoice(arg, value)) {
            push(
              'bad_choice',
              '`' + node.key + '` 的位置参数只能取 ' + choicesText(arg) + '，不是 `' + value + '`。',
            );
          }
        });
      }

      for (const arg of node.args) {
        if (arg.required && !arg.positional && !seen.has(arg.id)) {
          push('missing_required', '`' + node.key + '` 必须带 `' + flagLabel(arg) + '`。');
        }
      }
      // An ArgGroup is the part of the grammar that is invisible from the
      // arguments themselves: `check`'s target is required even though neither
      // member is, and `multiple: false` is what makes `check name --client x`
      // a conflict rather than two perfectly good options.
      for (const group of node.argGroups || []) {
        const ids = group.oneOf || [];
        if (!ids.length) continue;
        const present = ids.filter(function (id) {
          return seen.has(id);
        });
        if (present.length === 0) {
          push('missing_group', groupLabel(node, ids) + '至少要给其中一个。');
        } else if (group.multiple !== true && present.length > 1) {
          push('group_conflict', groupLabel(node, ids) + '只能给一个。');
        }
      }
      if (usedJsonFlag && node.jsonSupported !== true) {
        push('no_json', '`' + node.key + '` 没有 --json。');
      }
      if (usedSecretFlag && !(node.secretRequired || []).length) {
        push('no_secrets', '`' + node.key + '` 不需要任何凭据字段，--secrets-stdin 是多余的。');
      }
      return {
        ok: errors.length === 0,
        errors,
        node,
        command: node.key,
        positionals,
        given,
        usedJsonFlag,
        usedSecretFlag,
      };
    }

    function valueNames(declared) {
      return declared
        .map(function (arg) {
          return (arg.valueNames && arg.valueNames[0]) || arg.id.toUpperCase();
        })
        .join(' ');
    }

    function groupLabel(node, ids) {
      const labels = ids.map(function (id) {
        const arg = node.args.find(function (item) {
          return item.id === id;
        });
        if (!arg) return '`' + id + '`';
        return arg.positional
          ? '`' + ((arg.valueNames && arg.valueNames[0]) || arg.id.toUpperCase()) + '`'
          : '`' + flagLabel(arg) + '`';
      });
      return '`' + node.key + '` 的 ' + labels.join(' 或 ') + ' ';
    }

    // Suggestion list for the practice input: command names plus typed prefix.
    function complete(prefix) {
      let tokens;
      try {
        tokens = tokenize(prefix);
      } catch (error) {
        return [];
      }
      if (tokens.length && PREFIXES.includes(tokens[0])) tokens = tokens.slice(1);
      // A trailing space means the next word is being started, so every token so
      // far names the command and there is no partial word to filter on yet.
      const openWord = /\s$/.test(prefix);
      const last = openWord || !tokens.length ? '' : tokens[tokens.length - 1];
      const head = openWord ? tokens : tokens.slice(0, -1);
      // Suggestions only make sense while everything before the cursor is a
      // command name or a complete global flag; once a value has been started
      // there is nothing left the grammar can offer.
      const found = head.length ? resolve(head) : null;
      if (head.length && (!found || !found.node || found.rest.length)) return [];
      const node = found && found.node;
      const pool = [];
      if (!node) {
        for (const item of rules.commands) if (!item.parent) pool.push(item.key);
      } else {
        // Bare names only: the practice input replaces the word being typed.
        for (const item of rules.commands) {
          if (item.parent === node.key) pool.push(item.key.split(' ').pop());
        }
        for (const arg of node.args.concat(globals)) {
          if (arg.positional) continue;
          if (arg.long) pool.push('--' + arg.long);
          for (const alias of arg.aliases || []) pool.push('--' + alias);
          if (arg.short && !arg.takesValue) pool.push('-' + arg.short);
        }
      }
      const seen = new Set(pool);
      const candidates = [...seen];
      if (!last) return candidates.slice(0, 12);
      return candidates.filter(function (item) {
        return item.startsWith(last);
      }).slice(0, 12);
    }

    return { validate, resolve, complete, tokenize, byKey };
  }

  // gradeDrill(eng, line, drill) -> { ok, problems, result }
  //
  // A drill says which command it wants plus the flags it expects, forbids and
  // requires a specific value for. Short options and aliases count as their
  // long form, because that is what the real binary does.
  function gradeDrill(eng, line, drill) {
    const result = eng.validate(line, { requirePrefix: false });
    const problems = result.errors.map(function (issue) {
      return issue.message;
    });
    if (!result.node) return { ok: false, problems, result };
    const expect = drill.expect || {};
    const given = result.given || [];
    const matchesFlag = (item, flag) => (item.long || item.raw) === flag || item.raw === flag;
    const hasFlag = (flag) => given.some(function (item) { return matchesFlag(item, flag); });
    const valueOf = function (flag) {
      const hits = given.filter(function (item) { return matchesFlag(item, flag); });
      return hits.length ? hits[hits.length - 1].value : undefined;
    };
    if (expect.key && result.command !== expect.key) {
      problems.push('命令落在 `' + result.command + '`，这一关要的是 `' + expect.key + '`。');
    }
    for (const flag of expect.flags || []) {
      if (!hasFlag(flag)) problems.push('还缺 `' + flag + '`。');
    }
    for (const flag of expect.forbid || []) {
      if (hasFlag(flag)) problems.push('这一关不该出现 `' + flag + '`。');
    }
    for (const flag of expect.mustAbsent || []) {
      if (hasFlag(flag)) problems.push('这条不能带 `' + flag + '`。');
    }
    const pairs = expect.flagValues || {};
    for (const flag of Object.keys(pairs)) {
      const value = valueOf(flag);
      if (value !== undefined && value !== pairs[flag]) {
        problems.push('`' + flag + '` 这里要填 `' + pairs[flag] + '`，不是 `' + value + '`。');
      }
    }
    if (typeof expect.positionals === 'number') {
      const got = (result.positionals || []).length;
      if (got !== expect.positionals) {
        problems.push('位置参数要 ' + expect.positionals + ' 个，现在 ' + got + ' 个。');
      }
    }
    return { ok: problems.length === 0, problems, result };
  }

  return { create, tokenize, gradeDrill, PREFIXES };
});
