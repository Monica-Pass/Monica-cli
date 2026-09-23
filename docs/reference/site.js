// Renders docs/reference/data.js into a three-panel teaching site.
//
// Nothing here executes a command: the practice mode grades typed lines with
// the same grammar checker the build uses (grammar.js), against the grammar
// snapshot recorded in data.js.

(function () {
  'use strict';

  const DATA = window.MONICA_TEACH;
  const GRAMMAR = window.MONICA_GRAMMAR;
  if (!DATA || !GRAMMAR) {
    document.body.innerHTML =
      '<p style="padding:40px;font-family:monospace">data.js 没加载到。请确认 index.html 与 data.js、grammar.js、site.js 在同一个目录。</p>';
    return;
  }

  const engine = GRAMMAR.create({
    cli: DATA.meta.cli,
    commands: DATA.commands,
    globals: DATA.meta.globals,
  });
  const byKey = engine.byKey;
  const GROUPS = DATA.meta.groups || [];
  const GROUP_LABEL = new Map(GROUPS.map((g) => [g.id, g.label]));

  const STORE_DRILL = 'monica-teach:drills';
  const STORE_THEME = 'monica-teach:theme';

  const state = {
    view: 'table',
    group: 'all',
    marks: new Set(),
    search: '',
    key: DATA.commands[0].key,
    mode: 'drill',
    drill: 0,
    tries: 0,
    done: readJson(STORE_DRILL, []),
  };

  // ------------------------------ helpers ---------------------------------

  function readJson(name, fallback) {
    try {
      const raw = localStorage.getItem(name);
      return raw ? JSON.parse(raw) : fallback;
    } catch (error) {
      return fallback;
    }
  }

  function writeJson(name, value) {
    try {
      localStorage.setItem(name, JSON.stringify(value));
    } catch (error) {
      /* private mode: progress just stays in memory */
    }
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  function clear(node) {
    while (node.firstChild) node.removeChild(node.firstChild);
    return node;
  }

  const $ = (id) => document.getElementById(id);

  function flagList(cmd) {
    return cmd.args
      .filter((arg) => !arg.positional)
      .map((arg) => (arg.long ? '--' + arg.long : '-' + arg.short));
  }

  function cmdChips(cmd) {
    const chips = [];
    if (cmd.secretRequired.length) chips.push(['chip-secret', '需凭据']);
    if (cmd.jsonSupported) chips.push(['chip-json', '--json']);
    if (cmd.longRunning) chips.push(['chip-long', '长驻']);
    if (cmd.handAuthored) chips.push(['chip-hand', '人工条目']);
    return chips;
  }

  function chipRow(chips) {
    const row = el('div', 'chip-row');
    for (const [cls, text] of chips) row.appendChild(el('span', 'chip ' + cls, text));
    if (!chips.length) row.appendChild(el('span', 'chip', '—'));
    return row;
  }

  // ------------------------------- table ----------------------------------

  function matches(cmd) {
    if (state.group !== 'all' && cmd.group !== state.group) return false;
    if (state.marks.has('secret') && !cmd.secretRequired.length) return false;
    if (state.marks.has('json') && !cmd.jsonSupported) return false;
    if (state.marks.has('long') && !cmd.longRunning) return false;
    if (state.marks.has('hand') && !cmd.handAuthored) return false;
    const q = state.search.trim().toLowerCase();
    if (!q) return true;
    const haystack = [
      cmd.key,
      cmd.summaryZh,
      cmd.summary,
      cmd.whenToUse,
      cmd.aliases.join(' '),
      flagList(cmd).join(' '),
      cmd.args.map((arg) => arg.help).join(' '),
      cmd.pitfalls.join(' '),
    ]
      .join(' ')
      .toLowerCase();
    return q.split(/\s+/).every((piece) => haystack.indexOf(piece) >= 0);
  }

  function renderToolbar() {
    const segs = clear($('group-segments'));
    const all = el('button', 'seg' + (state.group === 'all' ? ' is-on' : ''), '全部');
    all.type = 'button';
    all.onclick = () => {
      state.group = 'all';
      renderTable();
    };
    segs.appendChild(all);
    for (const group of GROUPS) {
      const count = DATA.commands.filter((cmd) => cmd.group === group.id).length;
      const button = el(
        'button',
        'seg' + (state.group === group.id ? ' is-on' : ''),
        group.label + ' ' + String(count).padStart(2, '0'),
      );
      button.type = 'button';
      button.title = group.hint || '';
      button.onclick = () => {
        state.group = group.id;
        renderTable();
      };
      segs.appendChild(button);
    }

    const marks = clear($('flag-segments'));
    const markDefs = [
      ['secret', '需凭据'],
      ['json', '支持 --json'],
      ['long', '长驻进程'],
      ['hand', '不在机器语法里'],
    ];
    for (const [id, label] of markDefs) {
      const on = state.marks.has(id);
      const button = el('button', 'seg' + (on ? ' is-accent' : ''), label);
      button.type = 'button';
      button.onclick = () => {
        if (on) state.marks.delete(id);
        else state.marks.add(id);
        renderTable();
      };
      marks.appendChild(button);
    }
  }

  function renderTable() {
    renderToolbar();
    const rows = clear($('cmd-rows'));
    const list = DATA.commands.filter(matches);
    $('table-empty').hidden = list.length > 0;
    $('cmd-table').hidden = list.length === 0;
    $('table-count').textContent =
      list.length + ' / ' + DATA.commands.length + ' 条 · 语法内 ' + DATA.meta.commandCount + ' 条';
    for (const cmd of list) {
      const tr = el('tr');
      if (cmd.key === state.key) tr.className = 'is-open';
      tr.appendChild(el('td', 'cmd-key', DATA.meta.cli + ' ' + cmd.key));
      tr.appendChild(el('td', 'cmd-alias', cmd.aliases.join(', ')));
      const sum = el('td', 'cmd-sum');
      sum.appendChild(document.createTextNode(cmd.summaryZh));
      sum.appendChild(el('small', null, cmd.summary));
      tr.appendChild(sum);
      const td = el('td');
      td.appendChild(chipRow(cmdChips(cmd)));
      tr.appendChild(td);
      tr.onclick = () => openDetail(cmd.key);
      rows.appendChild(tr);
    }
  }

  // ------------------------------- detail ---------------------------------

  function usageLine(cmd) {
    const parts = [DATA.meta.cli + ' ' + cmd.key];
    const requiredFlags = cmd.args.filter((arg) => arg.required && !arg.positional);
    if (cmd.args.some((arg) => !arg.positional && !arg.required)) parts.push('[OPTIONS]');
    for (const arg of cmd.args.filter((arg) => arg.positional)) {
      const name = (arg.valueNames && arg.valueNames[0]) || arg.id.toUpperCase();
      parts.push(arg.required ? '<' + name + '>' : '[' + name + ']');
    }
    for (const arg of requiredFlags) {
      parts.push((arg.long ? '--' + arg.long : '-' + arg.short) + ' <' + ((arg.valueNames && arg.valueNames[0]) || 'VALUE') + '>');
    }
    return parts.join(' ');
  }

  // An ArgGroup member is neither optional nor required on its own, which the
  // per-argument `required` flag cannot express; say so in the condition column.
  function groupMarks(groups) {
    const marks = new Map();
    for (const group of groups || []) {
      const text = group.multiple === true ? '组内至少一个' : '组内恰好一个';
      for (const id of group.oneOf || []) {
        if (!marks.has(id)) marks.set(id, text);
      }
    }
    return marks;
  }

  function argRow(arg, group) {
    const tr = el('tr');
    const name = el('td', 'arg-name');
    if (arg.positional) {
      name.textContent = '<' + ((arg.valueNames && arg.valueNames[0]) || arg.id.toUpperCase()) + '>';
    } else {
      const bits = [];
      if (arg.long) bits.push('--' + arg.long);
      if (arg.short) bits.push('-' + arg.short);
      for (const alias of arg.aliases || []) bits.push('--' + alias + '（别名）');
      name.textContent = bits.join('  ');
    }
    tr.appendChild(name);

    const value = el('td', 'flag-meta');
    if (arg.positional || !arg.takesValue) {
      value.textContent = arg.takesValue === false ? '开关' : '—';
    } else {
      value.textContent = (arg.valueNames && arg.valueNames.join(' ')) || '取值';
    }
    tr.appendChild(value);

    const meta = el('td', 'flag-meta arg-cond');
    const marks = [];
    if (arg.required) marks.push('必填');
    else if (group) marks.push(group);
    if (arg.repeatable) marks.push('可重复');
    if (arg.choices && arg.choices.length) {
      const aliases = arg.choiceAliases || [];
      marks.push(arg.choices.join('/') + (aliases.length ? '（别名 ' + aliases.join('/') + '）' : ''));
    }
    const defaults = (arg.defaults || []).filter((value) => value !== '');
    if (defaults.length) marks.push('默认 ' + defaults.join(','));
    meta.textContent = marks.join(' · ');
    tr.appendChild(meta);

    tr.appendChild(el('td', 'arg-help', arg.help || ''));
    return tr;
  }

  function argTable(args, groups) {
    const wrap = el('div', 'table-wrap');
    const table = el('table', 'arg-table');
    const head = el('tr');
    for (const label of ['名称', '取值', '条件', '说明']) head.appendChild(el('th', null, label));
    const thead = el('thead');
    thead.appendChild(head);
    table.appendChild(thead);
    const body = el('tbody');
    const marks = groupMarks(groups);
    for (const arg of args) body.appendChild(argRow(arg, marks.get(arg.id)));
    table.appendChild(body);
    wrap.appendChild(table);
    return wrap;
  }

  function exampleCard(cmd, example, index) {
    const card = el('div', 'example');
    const line = el('div', 'example-cmd');
    line.appendChild(el('code', null, example.cmd));
    const copy = el('button', 'pill pill-ghost', '复制');
    copy.type = 'button';
    copy.onclick = () => {
      copyText(example.cmd, copy);
    };
    line.appendChild(copy);
    card.appendChild(line);

    if (example.note) card.appendChild(el('div', 'example-note', example.note));
    if (example.secrets) {
      const secrets = el('div', 'example-secrets');
      secrets.appendChild(el('span', null, 'stdin 载荷 '));
      secrets.appendChild(document.createTextNode(example.secrets));
      card.appendChild(secrets);
    }

    if (typeof example.out === 'string' && example.out.trim()) {
      card.appendChild(el('pre', 'out', example.out));
    }

    const foot = el('div', 'example-foot');
    if (example.tested === true) {
      const chips = el('div', 'chip-row');
      chips.appendChild(el('span', 'chip chip-ok', '实测输出'));
      chips.appendChild(el('span', 'chip', '0.5.0'));
      foot.appendChild(chips);
    } else {
      const chips = el('div', 'chip-row');
      chips.appendChild(el('span', 'chip chip-hand', '未采集'));
      foot.appendChild(chips);
    }
    const why = el('span', 'why', example.tested === true ? '' : example.reason || '');
    foot.appendChild(why);
    const practice = el('button', 'linklike', '拿到练习里检查');
    practice.type = 'button';
    practice.onclick = () => {
      gotoPractice('free', example.cmd);
    };
    foot.appendChild(practice);
    card.appendChild(foot);
    return card;
  }

  function markScrollable(host) {
    for (const wrap of host.querySelectorAll('.table-wrap')) {
      wrap.classList.toggle('is-scrollable', wrap.scrollWidth > wrap.clientWidth + 1);
    }
  }

  function renderDetail() {
    const cmd = byKey.get(state.key) || DATA.commands[0];
    state.key = cmd.key;
    const host = clear($('detail-body'));

    const head = el('div', 'detail-head');
    head.appendChild(
      el(
        'div',
        'detail-path',
        (GROUP_LABEL.get(cmd.group) || '命令') +
          '  /  ' +
          (cmd.parent ? cmd.parent + '  /  ' : '') +
          cmd.name +
          (cmd.aliases.length ? '   别名 ' + cmd.aliases.join(', ') : ''),
      ),
    );
    head.appendChild(el('h1', 'detail-title', DATA.meta.cli + ' ' + cmd.key));
    head.appendChild(el('p', 'detail-zh', cmd.summaryZh));
    head.appendChild(el('p', 'detail-en', cmd.summary));
    if (cmd.whenToUse) head.appendChild(el('p', 'detail-when', cmd.whenToUse));
    head.appendChild(chipRow(cmdChips(cmd)));
    head.appendChild(el('div', 'usage-line', usageLine(cmd)));
    host.appendChild(head);

    const sections = el('div', 'sections');

    const argsPanel = el('div', 'panel');
    argsPanel.appendChild(el('h2', 'panel-title', '参数与选项'));
    const own = cmd.args;
    if (own.length) argsPanel.appendChild(argTable(own, cmd.argGroups));
    else argsPanel.appendChild(el('p', 'pane-hint', '这条命令没有自有参数，只用全局参数。'));
    if (cmd.secretRequired.length) {
      const secret = el('p', 'pane-hint');
      secret.appendChild(
        document.createTextNode(
          '需要凭据字段：' + cmd.secretRequired.join('、') + '。终端里隐式提示输入；自动化走 --secrets-stdin，一次一条命令、值不外泄。',
        ),
      );
      argsPanel.appendChild(secret);
    }
    const details = el('details', 'global-args');
    details.appendChild(el('summary', null, '全局参数（每条命令都能用）'));
    details.appendChild(argTable(DATA.meta.globals));
    argsPanel.appendChild(details);
    sections.appendChild(argsPanel);

    const examplesPanel = el('div', 'panel');
    examplesPanel.appendChild(el('h2', 'panel-title', '示例 · ' + cmd.examples.length + ' 条'));
    examplesPanel.appendChild(el('p', 'pane-hint', DATA.notes.normalised));
    cmd.examples.forEach((example, index) => {
      examplesPanel.appendChild(exampleCard(cmd, example, index));
    });
    sections.appendChild(examplesPanel);

    if (cmd.pitfalls.length) {
      const pit = el('div', 'panel');
      pit.appendChild(el('h2', 'panel-title', '容易踩的地方'));
      const ul = el('ul', 'pitfalls');
      for (const item of cmd.pitfalls) ul.appendChild(el('li', null, item));
      pit.appendChild(ul);
      sections.appendChild(pit);
    }

    if (cmd.handAuthored) {
      const note = el('div', 'panel');
      note.appendChild(el('h2', 'panel-title', '为什么这条不在机器语法里'));
      note.appendChild(
        el(
          'p',
          'pane-hint',
          '它不出现在 monica commands --json 中：这是故意留给人的一条出口。这里的参数表是手写并对着 --help 校过的，练习模式按那份手写字典判分。',
        ),
      );
      sections.appendChild(note);
    }

    host.appendChild(sections);

    const nav = el('div', 'detail-nav');
    const list = DATA.commands.filter(matches);
    const at = list.findIndex((item) => item.key === cmd.key);
    const prev = el('button', 'pill', '← 上一条');
    prev.type = 'button';
    prev.disabled = at <= 0;
    prev.onclick = () => openDetail(list[at - 1].key);
    const next = el('button', 'pill', '下一条 →');
    next.type = 'button';
    next.disabled = at < 0 || at >= list.length - 1;
    next.onclick = () => openDetail(list[at + 1].key);
    nav.appendChild(prev);
    nav.appendChild(el('span', 'mono-label', (at >= 0 ? at + 1 : '?') + ' / ' + list.length));
    nav.appendChild(next);
    host.appendChild(nav);
    markScrollable(host);
  }

  // ------------------------------- practice -------------------------------

  function grade(line, drill) {
    return GRAMMAR.gradeDrill(engine, line, drill);
  }

  function renderProgress() {
    const total = DATA.drills.length;
    const done = state.done.length;
    $('drill-readout').textContent =
      String(done).padStart(2, '0') + '/' + String(total).padStart(2, '0');
    const bar = clear($('drill-bar'));
    DATA.drills.forEach((drill, index) => {
      const seg = el('i');
      if (state.done.indexOf(drill.id) >= 0) seg.className = 'is-done';
      if (index === state.drill) seg.className += (seg.className ? ' ' : '') + 'is-current';
      seg.title = drill.title;
      bar.appendChild(seg);
    });
  }

  function hintText(drill) {
    return (
      '第 ' +
      String(state.drill + 1).padStart(2, '0') +
      ' 关  /  monica ' +
      drill.key +
      (state.done.indexOf(drill.id) >= 0 ? '   [已过]' : '')
    );
  }

  function renderDrill() {
    renderProgress();
    const drill = DATA.drills[state.drill];
    const pick = clear($('drill-pick'));
    DATA.drills.forEach((item, index) => {
      const option = el('option', null, String(index + 1).padStart(2, '0') + '  ' + item.title);
      option.value = String(index);
      if (index === state.drill) option.selected = true;
      pick.appendChild(option);
    });

    const card = clear($('drill-card'));
    card.appendChild(
      el('div', 'drill-hintline', hintText(drill)),
    );
    card.appendChild(el('h2', 'drill-scene', drill.title));
    card.appendChild(el('p', 'drill-prompt', drill.prompt));

    const label = el('label', 'field');
    label.appendChild(el('span', 'field-label', '你的命令'));
    const input = el('input');
    input.type = 'text';
    input.spellcheck = false;
    input.autocomplete = 'off';
    input.id = 'drill-input';
    input.placeholder = 'monica ' + drill.key + ' …';
    label.appendChild(input);
    card.appendChild(label);

    const actions = el('div', 'drill-actions');
    const check = el('button', 'pill pill-accent', '检查');
    check.type = 'button';
    check.onclick = () => submitDrill(drill);
    const show = el('button', 'pill pill-ghost', '看答案');
    show.type = 'button';
    show.onclick = () => revealDrill(drill);
    const detail = el('button', 'linklike', '打开这条命令的说明');
    detail.type = 'button';
    detail.onclick = () => openDetail(drill.key);
    actions.appendChild(check);
    actions.appendChild(show);
    actions.appendChild(detail);
    card.appendChild(actions);
    card.appendChild(el('div', 'verdict', ''));

    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        submitDrill(drill);
      }
    });
    $('mode-drill').classList.add('is-active');
  }

  function submitDrill(drill) {
    const input = $('drill-input');
    const line = (input.value || '').trim();
    const verdict = $('drill-card').querySelector('.verdict');
    state.tries += 1;
    const graded = grade(line, drill);
    clear(verdict);
    input.classList.toggle('is-bad', !graded.ok);
    if (graded.ok) {
      verdict.className = 'verdict ok';
      verdict.appendChild(el('h3', null, '[ 通过 ] 语法与意图都对'));
      verdict.appendChild(el('div', 'answer', line));
      verdict.appendChild(el('p', 'why', drill.why));
      if (state.done.indexOf(drill.id) < 0) {
        state.done.push(drill.id);
        writeJson(STORE_DRILL, state.done);
        renderProgress();
        $('drill-card').querySelector('.drill-hintline').textContent = hintText(drill);
      }
      state.tries = 0;
      return;
    }
    verdict.className = 'verdict bad';
    verdict.appendChild(el('h3', null, '[ 还不对 ] 第 ' + state.tries + ' 次尝试'));
    const ul = el('ul');
    for (const problem of graded.problems) ul.appendChild(el('li', null, problem));
    verdict.appendChild(ul);
    if (state.tries >= 2) {
      const hint = el('p', 'why', '卡住了？按「看答案」，它会连原因一起给你。');
      verdict.appendChild(hint);
    }
  }

  function revealDrill(drill) {
    const verdict = $('drill-card').querySelector('.verdict');
    clear(verdict);
    verdict.className = 'verdict';
    verdict.appendChild(el('h3', null, '[ 答案 ]'));
    verdict.appendChild(el('div', 'answer', drill.answer));
    verdict.appendChild(el('p', 'why', drill.why));
    const input = $('drill-input');
    if (input) {
      input.value = drill.answer;
      input.classList.remove('is-bad');
    }
  }

  function renderFree(prefill) {
    const input = $('free-input');
    if (prefill !== undefined) input.value = prefill;
    $('mode-free').classList.add('is-active');
    renderSuggest(input.value);
    renderFreeVerdict();
  }

  function renderSuggest(line) {
    const host = clear($('free-suggest'));
    const items = engine.complete(line || '').slice(0, 10);
    if (!items.length) {
      host.appendChild(el('span', 'drill-hintline', '（补全为空：先敲 monica，或换个前缀）'));
      return;
    }
    for (const item of items) {
      const button = el('button', null, item);
      button.type = 'button';
      button.onclick = () => {
        const field = $('free-input');
        const parts = field.value.split(/\s/);
        parts[parts.length - 1] = item;
        field.value = parts.join(' ').replace(/\s{2,}/g, ' ');
        renderSuggest(field.value);
        field.focus();
      };
      host.appendChild(button);
    }
  }

  function renderFreeVerdict() {
    const host = clear($('free-verdict'));
    const line = ($('free-input').value || '').trim();
    if (!line) {
      host.className = 'verdict';
      host.appendChild(el('p', 'why', '敲完按回车，或点「检查」。'));
      return;
    }
    const result = engine.validate(line, { requirePrefix: false });
    const cmd = result.node ? byKey.get(result.command) : null;
    host.className = 'verdict ' + (result.ok ? 'ok' : 'bad');
    if (result.ok) {
      host.appendChild(
        el('h3', null, '[ 语法通过 ] ' + (result.command ? DATA.meta.cli + ' ' + result.command : '只到根命令')),
      );
      host.appendChild(el('div', 'answer', line));
      // A line can be well formed and still be worth a word of explanation —
      // `monica --help` prints help, it does not operate on the vault.
      for (const issue of result.errors) {
        host.appendChild(el('p', 'why', issue.message));
      }
      // A help line never reaches the vault, so the credential reminder would
      // only be noise under a verdict that says exactly that.
      const printsOnly = result.errors.some(function (issue) {
        return issue.code === 'help_shortcut' || issue.code === 'root_version';
      });
      if (cmd) {
        host.appendChild(el('p', 'why', cmd.summaryZh));
        if (!printsOnly && cmd.secretRequired.length) {
          host.appendChild(
            el('p', 'why', '这条要凭据字段：' + cmd.secretRequired.join('、') + '。'),
          );
        }
        if (!printsOnly && !result.usedSecretFlag && cmd.secretRequired.length) {
          host.appendChild(el('p', 'why', '在终端里会隐式提示；自动化才用 --secrets-stdin。'));
        }
        const link = el('button', 'linklike', '看完整说明与实测输出');
        link.type = 'button';
        link.onclick = () => openDetail(cmd.key);
        host.appendChild(link);
      }
      return;
    }
    host.appendChild(el('h3', null, '[ 语法不通过 ]'));
    const ul = el('ul');
    for (const issue of result.errors) ul.appendChild(el('li', null, issue.message));
    host.appendChild(ul);
    if (cmd) {
      host.appendChild(el('p', 'why', '参考：' + cmd.examples[0].cmd));
    }
  }

  function gotoPractice(mode, prefill) {
    state.mode = mode;
    if (mode === 'free') {
      setView('practice');
      setMode('free');
      renderFree(prefill);
      location.hash = '#practice/free';
    } else {
      setView('practice');
      setMode('drill');
    }
  }

  // ------------------------------- shell ----------------------------------

  function setView(view) {
    state.view = view;
    for (const node of document.querySelectorAll('.view')) {
      node.classList.toggle('is-active', node.id === 'view-' + view);
    }
    for (const node of document.querySelectorAll('.viewnav-item')) {
      node.classList.toggle('is-active', node.dataset.view === view);
    }
    if (view === 'detail') renderDetail();
    if (view === 'practice') {
      renderDrill();
      renderFree();
      setMode(state.mode);
    }
    markScrollable(document);
  }

  function setMode(mode) {
    state.mode = mode;
    for (const node of document.querySelectorAll('.mode-item')) {
      node.classList.toggle('is-active', node.dataset.mode === mode);
    }
    $('mode-drill').classList.toggle('is-active', mode === 'drill');
    $('mode-free').classList.toggle('is-active', mode === 'free');
  }

  function openDetail(key) {
    if (!byKey.has(key)) return;
    state.key = key;
    setView('detail');
    location.hash = '#detail/' + encodeURIComponent(key);
    window.scrollTo({ top: 0 });
  }

  function copyText(text, button) {
    const done = () => {
      const label = button.textContent;
      button.textContent = '[ 已复制 ]';
      button.classList.add('copied');
      window.setTimeout(() => {
        button.textContent = label;
        button.classList.remove('copied');
      }, 1200);
    };
    const fallback = () => {
      const area = el('textarea');
      area.value = text;
      area.style.position = 'fixed';
      area.style.opacity = '0';
      document.body.appendChild(area);
      area.select();
      try {
        document.execCommand('copy');
        done();
      } catch (error) {
        /* nothing else to try */
      }
      document.body.removeChild(area);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
  }

  function applyTheme(theme) {
    document.documentElement.dataset.theme = theme;
    $('theme-toggle').textContent = theme === 'dark' ? '[ 暗 ]' : '[ 亮 ]';
    writeJson(STORE_THEME, theme);
  }

  function routeFromHash() {
    const raw = decodeURIComponent((location.hash || '').replace(/^#/, ''));
    const [view, arg] = raw.split('/');
    if (view === 'practice') {
      if (arg === 'free') {
        setView('practice');
        setMode('free');
        return;
      }
      if (arg) {
        const index = DATA.drills.findIndex((drill) => drill.id === arg);
        if (index >= 0) state.drill = index;
      }
      setView('practice');
      setMode('drill');
      renderDrill();
      return;
    }
    if (view === 'detail' && arg && byKey.has(arg)) {
      state.key = arg;
      setView('detail');
      return;
    }
    setView('table');
  }

  // ------------------------------- wiring ---------------------------------

  $('version-chip').textContent = DATA.meta.version.toUpperCase();
  $('table-lede').textContent =
    DATA.meta.commandCount +
    ' 条命令来自 monica commands --json，' +
    DATA.commands.length +
    ' 个教学条目带示例与真实输出。点一行看详情，或直接去练习。';
  $('practice-boundary').textContent = DATA.notes.boundary;
  $('practice-limits').textContent = DATA.notes.grammarLimits;
  $('foot-note').textContent = DATA.notes.fixture;
  $('foot-meta').textContent =
    DATA.meta.generatedFrom + ' · ' + DATA.meta.version + ' · 语法 schema v' +
    DATA.meta.grammarVersion + ' · 生成于 ' + (DATA.generatedAt || '未知');

  for (const node of document.querySelectorAll('.viewnav-item')) {
    node.onclick = () => {
      const view = node.dataset.view;
      setView(view);
      location.hash = '#' + view;
    };
  }
  for (const node of document.querySelectorAll('.mode-item')) {
    node.onclick = () => setMode(node.dataset.mode);
  }
  $('search').addEventListener('input', (event) => {
    state.search = event.target.value;
    renderTable();
  });
  $('free-input').addEventListener('input', (event) => {
    renderSuggest(event.target.value);
    clear($('free-verdict'));
  });
  $('free-input').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      renderFreeVerdict();
    }
  });
  $('free-check').onclick = () => {
    renderFreeVerdict();
    $('free-input').focus();
  };
  $('drill-pick').addEventListener('change', (event) => {
    state.drill = Number(event.target.value) || 0;
    state.tries = 0;
    renderDrill();
    location.hash = '#practice/' + DATA.drills[state.drill].id;
  });
  $('drill-prev').onclick = () => {
    state.drill = (state.drill - 1 + DATA.drills.length) % DATA.drills.length;
    state.tries = 0;
    renderDrill();
    location.hash = '#practice/' + DATA.drills[state.drill].id;
  };
  $('drill-next').onclick = () => {
    state.drill = (state.drill + 1) % DATA.drills.length;
    state.tries = 0;
    renderDrill();
    location.hash = '#practice/' + DATA.drills[state.drill].id;
  };
  $('drill-reset').onclick = () => {
    state.done = [];
    state.drill = 0;
    state.tries = 0;
    writeJson(STORE_DRILL, state.done);
    renderDrill();
  };
  $('theme-toggle').onclick = () => {
    applyTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark');
  };
  window.addEventListener('hashchange', routeFromHash);
  window.addEventListener('resize', () => markScrollable(document));

  applyTheme(readJson(STORE_THEME, 'dark') === 'light' ? 'light' : 'dark');
  renderTable();
  routeFromHash();
  markScrollable(document);
})();
