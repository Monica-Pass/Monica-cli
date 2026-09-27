// Pixel quest for the Monica CLI practice book.
//
// Every level is a drill from data.js, graded by the same grammar checker the
// rest of the site uses (grammar.js). Nothing is executed. The pixel art is
// drawn at a small logical size on <canvas> and scaled up with nearest-neighbour
// sampling, so every sprite below is plain data: one character per pixel.

(function () {
  'use strict';

  const DATA = window.MONICA_TEACH;
  const GRAMMAR = window.MONICA_GRAMMAR;
  if (!DATA || !GRAMMAR) return;

  const engine = GRAMMAR.create({
    cli: DATA.meta.cli,
    commands: DATA.commands,
    globals: DATA.meta.globals,
  });
  const CMD = new Map(DATA.commands.map((cmd) => [cmd.key, cmd]));
  const GROUPS = DATA.meta.groups || [];
  const STORE = 'monica-teach:quest';
  const OLD_STORE = 'monica-teach:drills';
  const REDUCED = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ------------------------------ palette ---------------------------------
  // The 16-colour PICO-8 palette: bright, 8-bit, and readable on both themes
  // because the quest always sits inside its own dark "screen".
  const P = [
    '#000000', '#1d2b53', '#7e2553', '#008751', '#ab5236', '#5f574f', '#c2c3c7', '#fff1e8',
    '#ff004d', '#ffa300', '#ffec27', '#00e436', '#29adff', '#83769c', '#ff77a8', '#ffccaa',
  ];

  // Where each command group lives on the map, and what its scenes look like.
  const REGIONS = {
    start: { place: '起步村', sky: 12, ground: 3, deco: 'houses' },
    mdbx: { place: '磁盘矿坑', sky: 1, ground: 4, deco: 'cave' },
    vault: { place: '保险库城堡', sky: 13, ground: 5, deco: 'castle' },
    connections: { place: '信号塔', sky: 2, ground: 3, deco: 'hills' },
    grants: { place: '授权关口', sky: 12, ground: 4, deco: 'wall' },
    content: { place: '图书馆', sky: 4, ground: 4, deco: 'library' },
    sync: { place: '云端之桥', sky: 12, ground: 6, deco: 'clouds' },
  };

  // The scene each level plays when it is cleared; unknown ids fall back to the
  // region's own prop so a new drill never lands on an empty stage.
  const SCENES = {
    'first-vault': 'vault',
    'next-step': 'sign',
    unlock: 'wake',
    'disk-format': 'disk',
    'disk-files': 'crates',
    'tiga-look': 'shield',
    'lower-profile': 'shieldDown',
    'first-connection': 'tower',
    'self-hosted': 'tower',
    'retell-note': 'note',
    'scoped-grant': 'pass',
    'api-wide': 'pass',
    'approval-gate': 'bell',
    'install-claude': 'plug',
    rotate: 'hourglass',
    'cut-off': 'bars',
    'check-what-ai-sees': 'lens',
    trail: 'scroll',
    'find-id': 'books',
    shelve: 'books',
    'cloud-login': 'cloud',
    'cloud-sync': 'cloudSync',
  };
  const REGION_PROP = {
    start: 'sign', mdbx: 'disk', vault: 'shield', connections: 'tower',
    grants: 'pass', content: 'books', sync: 'cloud',
  };

  // ------------------------------ sprites ---------------------------------
  // '.' is transparent, a hex digit is a palette index.
  const MONICA_HEAD = [
    '....1111....',
    '...111111...',
    '..11111111..',
    '..11ffff11..',
    '..1ffffff1..',
    '..1f0ff0f1..',
    '..1ffffff1..',
    '...ffeeff...',
    '....ffff....',
  ];
  const MONICA_BLINK = MONICA_HEAD.map((row, i) => (i === 5 ? '..1f5ff5f1..' : row));
  const MONICA_BODY = [
    '..88877888..',
    '.f88887888f.',
    '.f88887888f.',
    '...888888...',
  ];
  const MONICA_CHEER_BODY = [
    'f.88877888.f',
    'f888887888f.',
    '..88887888..',
    '...888888...',
  ];
  const LEGS = {
    stand: ['...11..11...', '...11..11...', '..000..000..'],
    a: ['...11..11...', '..11....11..', '.000....000.'],
    b: ['....1111....', '....1111....', '....0000....'],
  };
  const ROBOT = [
    '.....a......',
    '.....6......',
    '..66666666..',
    '.6777777776.',
    '.67c7777c76.',
    '.6777777776.',
    '.6775555776.',
    '..66666666..',
    '...566665...',
    '.5566666655.',
    '.6.666666.6.',
    '...666666...',
    '...55..55...',
    '..555..555..',
  ];
  const ROBOT_ASLEEP = ROBOT.map((row) => row.replace(/c/g, '5').replace('a', '5'));

  function monica(pose, blink) {
    const head = blink ? MONICA_BLINK : MONICA_HEAD;
    const body = pose === 'cheer' ? MONICA_CHEER_BODY : MONICA_BODY;
    return head.concat(body, LEGS[pose === 'a' || pose === 'b' ? pose : 'stand']);
  }

  function sprite(ctx, rows, x, y, flip) {
    for (let r = 0; r < rows.length; r += 1) {
      const row = rows[r];
      for (let c = 0; c < row.length; c += 1) {
        const ch = row[flip ? row.length - 1 - c : c];
        if (ch === '.' || ch === ' ') continue;
        ctx.fillStyle = P[parseInt(ch, 16)];
        ctx.fillRect(Math.round(x) + c, Math.round(y) + r, 1, 1);
      }
    }
  }

  function rect(ctx, color, x, y, w, h) {
    ctx.fillStyle = P[color];
    ctx.fillRect(Math.round(x), Math.round(y), w, h);
  }

  // A deterministic generator, so trees stay put between visits.
  function rng(seed) {
    let s = seed >>> 0;
    return function () {
      s = (s * 1664525 + 1013904223) >>> 0;
      return s / 4294967296;
    };
  }

  // ------------------------------ helpers ---------------------------------

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  function button(className, text, onClick) {
    const node = el('button', className, text);
    node.type = 'button';
    node.onclick = onClick;
    return node;
  }

  function readState() {
    let saved = null;
    try {
      saved = JSON.parse(localStorage.getItem(STORE) || 'null');
    } catch (error) {
      saved = null;
    }
    const state = {
      stars: {},
      at: 0,
      introSeen: false,
      sound: false,
    };
    if (saved && typeof saved === 'object') {
      if (saved.stars && typeof saved.stars === 'object') state.stars = saved.stars;
      if (Number.isInteger(saved.at)) state.at = saved.at;
      state.introSeen = saved.introSeen === true;
      state.sound = saved.sound === true;
      return state;
    }
    // Drills cleared before the quest existed still count, at one star.
    try {
      const done = JSON.parse(localStorage.getItem(OLD_STORE) || '[]');
      if (Array.isArray(done)) for (const id of done) state.stars[id] = 1;
    } catch (error) {
      /* nothing to import */
    }
    return state;
  }

  const save = { stars: {}, at: 0, introSeen: false, sound: false };
  Object.assign(save, readState());

  function persist() {
    try {
      localStorage.setItem(STORE, JSON.stringify(save));
    } catch (error) {
      /* private mode: progress stays in memory */
    }
  }

  // ------------------------------- levels ---------------------------------
  // Drills are ordered by the region their command belongs to, keeping the
  // authored order inside a region.
  const groupIndex = new Map(GROUPS.map((group, index) => [group.id, index]));
  const LEVELS = DATA.drills
    .map((drill, order) => {
      const cmd = CMD.get(drill.key) || CMD.get(drill.expect && drill.expect.key);
      const region = (cmd && cmd.group) || 'start';
      return { drill, order, region };
    })
    .sort((a, b) => (groupIndex.get(a.region) ?? 99) - (groupIndex.get(b.region) ?? 99) || a.order - b.order)
    .map((level, index) => Object.assign(level, { index, number: index + 1 }));
  const LEVEL_BY_ID = new Map(LEVELS.map((level) => [level.drill.id, level]));
  if (save.at >= LEVELS.length) save.at = 0;

  function starsOf(level) {
    return save.stars[level.drill.id] || 0;
  }

  function totalStars() {
    return LEVELS.reduce((sum, level) => sum + starsOf(level), 0);
  }

  function clearedCount() {
    return LEVELS.filter((level) => starsOf(level) > 0).length;
  }

  function recommended() {
    const open = LEVELS.find((level) => starsOf(level) === 0);
    return open || null;
  }

  function starText(count) {
    return '★'.repeat(count) + '☆'.repeat(3 - count);
  }

  function regionLabel(id) {
    const group = GROUPS.find((item) => item.id === id);
    const region = REGIONS[id];
    return (region ? region.place : id) + (group ? ' · ' + group.label : '');
  }

  // -------------------------------- sound ---------------------------------
  const Sound = {
    ctx: null,
    tone(freq, length, type, volume, delay) {
      if (!save.sound) return;
      try {
        if (!this.ctx) this.ctx = new (window.AudioContext || window.webkitAudioContext)();
        const ctx = this.ctx;
        const start = ctx.currentTime + (delay || 0);
        const osc = ctx.createOscillator();
        const gain = ctx.createGain();
        osc.type = type || 'square';
        osc.frequency.setValueAtTime(freq, start);
        gain.gain.setValueAtTime(volume || 0.04, start);
        gain.gain.exponentialRampToValueAtTime(0.0001, start + length);
        osc.connect(gain).connect(ctx.destination);
        osc.start(start);
        osc.stop(start + length + 0.02);
      } catch (error) {
        /* no audio in this browser */
      }
    },
    step() { this.tone(520, 0.03, 'square', 0.02); },
    click() { this.tone(880, 0.03, 'square', 0.03); },
    good() { [523, 659, 784, 1047].forEach((f, i) => this.tone(f, 0.12, 'square', 0.04, i * 0.09)); },
    bad() { this.tone(150, 0.22, 'sawtooth', 0.035); },
    hint() { this.tone(740, 0.06, 'triangle', 0.05); this.tone(988, 0.08, 'triangle', 0.05, 0.07); },
  };

  // --------------------------------- map ----------------------------------
  const MAP_W = 320;
  const MAP_H = 180;
  // The road the levels sit on, sampled evenly so any number of drills fits.
  const ROAD = [
    [16, 160], [70, 162], [112, 150], [120, 122], [92, 104], [48, 100], [26, 80],
    [30, 52], [62, 30], [118, 24], [166, 38], [196, 64], [194, 96], [214, 124],
    [256, 142], [290, 124], [300, 90], [286, 52],
  ];

  function sampleRoad(count) {
    const lengths = [0];
    for (let i = 1; i < ROAD.length; i += 1) {
      const dx = ROAD[i][0] - ROAD[i - 1][0];
      const dy = ROAD[i][1] - ROAD[i - 1][1];
      lengths.push(lengths[i - 1] + Math.hypot(dx, dy));
    }
    const total = lengths[lengths.length - 1];
    const points = [];
    for (let n = 0; n < count; n += 1) {
      const target = ((n + 0.5) / count) * total;
      let i = 1;
      while (i < lengths.length - 1 && lengths[i] < target) i += 1;
      const t = (target - lengths[i - 1]) / (lengths[i] - lengths[i - 1] || 1);
      points.push([
        Math.round(ROAD[i - 1][0] + (ROAD[i][0] - ROAD[i - 1][0]) * t),
        Math.round(ROAD[i - 1][1] + (ROAD[i][1] - ROAD[i - 1][1]) * t),
      ]);
    }
    return points;
  }

  const NODES = sampleRoad(LEVELS.length);

  function regionCentre(id) {
    const points = LEVELS.filter((level) => level.region === id).map((level) => NODES[level.index]);
    if (!points.length) return null;
    const x = points.reduce((sum, p) => sum + p[0], 0) / points.length;
    const y = points.reduce((sum, p) => sum + p[1], 0) / points.length;
    return [x, y];
  }

  function nearRoad(x, y, margin) {
    for (let i = 1; i < ROAD.length; i += 1) {
      const [ax, ay] = ROAD[i - 1];
      const [bx, by] = ROAD[i];
      const dx = bx - ax;
      const dy = by - ay;
      const t = Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)));
      if (Math.hypot(ax + dx * t - x, ay + dy * t - y) < margin) return true;
    }
    return false;
  }

  function drawLandmark(ctx, kind, x, y) {
    x = Math.round(x);
    y = Math.round(y);
    switch (kind) {
      case 'houses':
        for (const [dx, roof] of [[0, 8], [14, 4]]) {
          rect(ctx, 7, x + dx, y + 6, 11, 8);
          for (let r = 0; r < 6; r += 1) rect(ctx, roof, x + dx + 5 - r, y + r, 1 + r * 2, 1);
          rect(ctx, 4, x + dx + 4, y + 10, 3, 4);
          rect(ctx, 12, x + dx + 1, y + 8, 2, 2);
        }
        break;
      case 'cave':
        rect(ctx, 5, x, y + 4, 22, 12);
        rect(ctx, 6, x + 2, y + 2, 18, 4);
        rect(ctx, 0, x + 6, y + 8, 10, 8);
        rect(ctx, 4, x + 5, y + 7, 12, 1);
        rect(ctx, 10, x + 9, y + 11, 3, 2);
        break;
      case 'castle':
        rect(ctx, 6, x, y + 4, 24, 14);
        for (let i = 0; i < 24; i += 4) rect(ctx, 6, x + i, y + 2, 2, 2);
        rect(ctx, 5, x + 2, y + 6, 20, 1);
        rect(ctx, 1, x + 9, y + 10, 6, 8);
        rect(ctx, 10, x + 11, y + 13, 2, 2);
        rect(ctx, 8, x + 20, y - 4, 1, 6);
        rect(ctx, 8, x + 21, y - 4, 3, 2);
        break;
      case 'hills':
        rect(ctx, 5, x + 8, y, 2, 16);
        rect(ctx, 5, x + 5, y + 14, 8, 2);
        rect(ctx, 6, x + 6, y + 6, 6, 1);
        rect(ctx, 6, x + 7, y + 3, 4, 1);
        rect(ctx, 8, x + 8, y - 2, 2, 2);
        break;
      case 'wall':
        for (let i = 0; i < 26; i += 1) {
          for (let j = 0; j < 12; j += 1) {
            rect(ctx, (i + (j % 2) * 2) % 4 === 0 ? 2 : 4, x + i, y + j, 1, 1);
          }
        }
        rect(ctx, 0, x + 9, y + 3, 8, 9);
        rect(ctx, 10, x + 10, y + 4, 6, 1);
        break;
      case 'library':
        rect(ctx, 4, x, y + 2, 22, 14);
        for (let i = 0; i < 3; i += 1) {
          rect(ctx, 0, x + 2, y + 4 + i * 4, 18, 1);
          for (let b = 0; b < 8; b += 1) rect(ctx, [8, 12, 11, 10, 14, 9, 13, 7][b], x + 3 + b * 2, y + 1 + i * 4 + 1, 1, 3);
        }
        break;
      case 'clouds':
        for (const [dx, dy] of [[0, 4], [8, 0], [16, 4]]) {
          rect(ctx, 7, x + dx, y + dy + 2, 10, 4);
          rect(ctx, 7, x + dx + 2, y + dy, 6, 8);
        }
        rect(ctx, 4, x - 2, y + 14, 30, 2);
        for (let i = 0; i < 30; i += 4) rect(ctx, 4, x - 2 + i, y + 12, 1, 4);
        break;
      default:
        break;
    }
  }

  // Where a region's landmark sits relative to the middle of its levels.
  const LANDMARK_OFFSET = {
    start: [-44, -26], mdbx: [6, -8], vault: [-8, 6], connections: [10, -2],
    grants: [-30, 6], content: [-8, -30], sync: [-38, -18],
  };

  function paintMap(ctx) {
    const random = rng(7);
    rect(ctx, 3, 0, 0, MAP_W, MAP_H);
    for (let i = 0; i < 1400; i += 1) {
      rect(ctx, random() < 0.5 ? 11 : 1, Math.floor(random() * MAP_W), Math.floor(random() * MAP_H), 1, 1);
    }
    // A lake and a river, never under the road.
    for (let y = 0; y < MAP_H; y += 1) {
      for (let x = 0; x < MAP_W; x += 1) {
        const lake = ((x - 150) / 26) ** 2 + ((y - 112) / 14) ** 2 < 1;
        const river = Math.abs(y - (132 + Math.sin(x / 18) * 6)) < 3 && x > 150 && x < 250;
        if ((lake || river) && !nearRoad(x, y, 7)) {
          rect(ctx, (x + y) % 9 === 0 ? 7 : 12, x, y, 1, 1);
        }
      }
    }
    // Trees, kept clear of the road and the landmarks.
    const centres = GROUPS.map((group) => {
      const centre = regionCentre(group.id);
      const offset = LANDMARK_OFFSET[group.id] || [0, 0];
      return centre ? [centre[0] + offset[0] + 12, centre[1] + offset[1] + 8] : null;
    }).filter(Boolean);
    for (let i = 0; i < 70; i += 1) {
      const x = Math.floor(random() * (MAP_W - 8));
      const y = Math.floor(random() * (MAP_H - 10)) + 2;
      if (nearRoad(x + 4, y + 6, 12)) continue;
      if (centres.some(([cx, cy]) => Math.hypot(cx - x, cy - y) < 22)) continue;
      if (((x - 150) / 30) ** 2 + ((y - 112) / 18) ** 2 < 1) continue;
      rect(ctx, 4, x + 3, y + 6, 2, 3);
      rect(ctx, 3, x, y + 2, 8, 4);
      rect(ctx, 3, x + 1, y, 6, 7);
      rect(ctx, 11, x + 2, y + 1, 2, 2);
    }
    // The road: a sand band with a darker edge.
    for (const [width, color] of [[7, 4], [5, 15]]) {
      for (let i = 1; i < ROAD.length; i += 1) {
        const [ax, ay] = ROAD[i - 1];
        const [bx, by] = ROAD[i];
        const steps = Math.ceil(Math.hypot(bx - ax, by - ay));
        for (let s = 0; s <= steps; s += 1) {
          const x = ax + ((bx - ax) * s) / steps;
          const y = ay + ((by - ay) * s) / steps;
          rect(ctx, color, x - Math.floor(width / 2), y - Math.floor(width / 2), width, width);
        }
      }
    }
    for (const group of GROUPS) {
      const centre = regionCentre(group.id);
      if (!centre) continue;
      const offset = LANDMARK_OFFSET[group.id] || [0, -24];
      drawLandmark(ctx, (REGIONS[group.id] || {}).deco, centre[0] + offset[0], centre[1] + offset[1]);
    }
  }

  // ------------------------------- scenes ---------------------------------
  const SCENE_W = 160;
  const SCENE_H = 96;
  const GROUND_Y = 74;

  function paintBackdrop(ctx, region, t) {
    const look = REGIONS[region] || REGIONS.start;
    rect(ctx, look.sky, 0, 0, SCENE_W, GROUND_Y);
    rect(ctx, look.ground, 0, GROUND_Y, SCENE_W, SCENE_H - GROUND_Y);
    for (let x = 0; x < SCENE_W; x += 4) rect(ctx, 0, x, GROUND_Y, 2, 1);
    const drift = Math.floor(t / 180) % SCENE_W;
    switch (look.deco) {
      case 'houses':
        rect(ctx, 10, 136, 8, 10, 10);
        for (const x of [10, 90]) {
          const cx = (x + drift) % (SCENE_W + 20) - 20;
          rect(ctx, 7, cx, 14, 16, 4);
          rect(ctx, 7, cx + 4, 11, 8, 8);
        }
        drawLandmark(ctx, 'houses', 6, GROUND_Y - 14);
        break;
      case 'cave':
        for (let i = 0; i < 40; i += 1) rect(ctx, 5, (i * 37) % SCENE_W, (i * 23) % 60, 3, 2);
        for (let x = 0; x < SCENE_W; x += 12) rect(ctx, 5, x, 0, 6, 6 + (x % 5));
        rect(ctx, 9, 20, 30, 2, 3);
        rect(ctx, 10, 20, 29 - ((t / 200) % 2 | 0), 2, 1);
        break;
      case 'castle':
        rect(ctx, 6, 0, 30, SCENE_W, GROUND_Y - 30);
        for (let x = 0; x < SCENE_W; x += 8) rect(ctx, 6, x, 26, 4, 4);
        for (let y = 34; y < GROUND_Y; y += 6) rect(ctx, 5, 0, y, SCENE_W, 1);
        break;
      case 'hills':
        for (let x = 0; x < SCENE_W; x += 1) {
          const h = 12 + Math.round(Math.sin(x / 14) * 6);
          rect(ctx, 1, x, GROUND_Y - h, 1, h);
        }
        for (let i = 0; i < 20; i += 1) rect(ctx, 7, (i * 29) % SCENE_W, (i * 11) % 30, 1, 1);
        break;
      case 'wall':
        for (let y = 20; y < GROUND_Y; y += 1) {
          for (let x = 0; x < SCENE_W; x += 1) {
            if ((x + ((Math.floor(y / 4) % 2) * 4)) % 8 === 0 || y % 4 === 0) rect(ctx, 2, x, y, 1, 1);
          }
        }
        rect(ctx, 4, 0, 20, SCENE_W, 1);
        break;
      case 'library':
        for (let shelf = 0; shelf < 3; shelf += 1) {
          rect(ctx, 0, 0, 14 + shelf * 20, SCENE_W, 2);
          for (let x = 2; x < SCENE_W; x += 3) {
            rect(ctx, [8, 12, 11, 10, 14, 9, 13][(x * 7 + shelf) % 7], x, 4 + shelf * 20, 2, 10);
          }
        }
        break;
      case 'clouds':
        for (const [x, y] of [[10, 12], [70, 26], [120, 8]]) {
          const cx = (x + drift) % (SCENE_W + 30) - 30;
          rect(ctx, 7, cx, y + 2, 24, 5);
          rect(ctx, 7, cx + 5, y, 14, 9);
        }
        break;
      default:
        break;
    }
  }

  // A prop draws its idle state at p = 0 and its win animation as p runs to 1.
  const PROPS = {
    vault(ctx, p, t) {
      const x = 96;
      rect(ctx, 5, x, 34, 34, 40);
      rect(ctx, 6, x + 2, 36, 30, 36);
      const open = Math.min(1, p * 1.6);
      if (open > 0) {
        rect(ctx, 10, x + 4, 38, 26, 32);
        for (let i = 0; i < 5; i += 1) rect(ctx, 9, x + 6 + i * 5, 58 - (i % 2) * 3, 4, 3);
      }
      const width = Math.round(26 * (1 - open));
      if (width > 0) {
        rect(ctx, 13, x + 4, 38, width, 32);
        const cx = x + 4 + width / 2;
        rect(ctx, 7, cx - 4, 50, 8, 8);
        rect(ctx, 5, cx - 1, 49 + Math.round(Math.sin(t / 150) * 0), 2, 10);
      }
    },
    sign(ctx, p, t) {
      rect(ctx, 4, 112, 36, 3, 38);
      const lit = p > 0.2;
      rect(ctx, lit ? 10 : 15, 98, 36, 30, 10);
      rect(ctx, 4, 98, 36, 30, 1);
      for (let i = 0; i < 4; i += 1) rect(ctx, lit ? 8 : 4, 126 + i, 37 + i, 1, 8 - i * 2);
      if (lit) for (let i = 0; i < 3; i += 1) rect(ctx, 0, 103 + i * 7, 40, 4, 2);
      rect(ctx, lit ? 11 : 15, 100, 50, 22, 8);
      if (p > 0.5 && Math.floor(t / 200) % 2) rect(ctx, 10, 130, 30, 2, 2);
    },
    wake(ctx, p, t) {
      sprite(ctx, p > 0.3 ? ROBOT : ROBOT_ASLEEP, 104, GROUND_Y - 14);
      if (p === 0) {
        const z = Math.floor(t / 400) % 3;
        for (let i = 0; i <= z; i += 1) rect(ctx, 7, 118 + i * 4, 52 - i * 5, 3, 1);
      } else if (p > 0.3) {
        for (let i = 0; i < 3; i += 1) rect(ctx, 10, 108 + i * 6, 44 - Math.round(p * 8), 1, 3);
      }
    },
    disk(ctx, p) {
      rect(ctx, 1, 98, 44, 30, 30);
      rect(ctx, 6, 104, 44, 18, 10);
      rect(ctx, 0, 116, 46, 3, 6);
      rect(ctx, 7, 102, 58, 22, 14);
      if (p > 0) {
        const y = 44 + Math.round(((p * 3) % 1) * 30);
        rect(ctx, 11, 94, y, 38, 1);
        if (p > 0.7) rect(ctx, 11, 104, 62, 14, 2);
      }
    },
    crates(ctx, p) {
      const shown = p > 0 ? Math.min(3, Math.ceil(p * 4)) : 1;
      for (let i = 0; i < shown; i += 1) {
        const x = 92 + i * 16;
        const size = 12 - i * 3;
        rect(ctx, 4, x, GROUND_Y - size, 12, size);
        rect(ctx, 9, x + 1, GROUND_Y - size + 1, 10, 1);
        rect(ctx, 0, x + 5, GROUND_Y - size, 2, size);
      }
    },
    shield(ctx, p) {
      for (let i = 0; i < 3; i += 1) {
        const lit = p > 0 ? p * 3 > i : i === 0;
        rect(ctx, lit ? 12 : 5, 100 + i * 12, 60 - i * 10, 10, 14 + i * 10);
      }
      rect(ctx, 7, 102, 40, 30, 2);
    },
    shieldDown(ctx, p) {
      for (let i = 0; i < 3; i += 1) {
        const lit = p > 0 ? i < 3 - Math.ceil(p * 1.5) : true;
        rect(ctx, lit ? 12 : 5, 100 + i * 12, 60 - i * 10, 10, 14 + i * 10);
      }
      if (p > 0.5) {
        rect(ctx, 7, 136, 40, 14, 18);
        for (let i = 0; i < 4; i += 1) rect(ctx, 5, 138, 43 + i * 4, 10, 1);
      }
    },
    tower(ctx, p, t) {
      for (let y = 0; y < 40; y += 1) rect(ctx, 5, 114 - Math.floor(y / 5), GROUND_Y - y, 2 + Math.floor(y / 5) * 2, 1);
      rect(ctx, 8, 113, 30, 4, 4);
      if (p > 0) {
        const ring = Math.floor(t / 160) % 4;
        for (let r = 0; r <= ring; r += 1) {
          const size = 6 + r * 6;
          rect(ctx, 11, 115 - size, 32 - r * 2, 2, 4);
          rect(ctx, 11, 115 + size, 32 - r * 2, 2, 4);
        }
      }
    },
    note(ctx, p) {
      sprite(ctx, ROBOT, 128, GROUND_Y - 14);
      const x = 70 + Math.round(p * 46);
      const y = 44 - Math.round(Math.sin(p * Math.PI) * 14);
      rect(ctx, 7, x, y, 12, 14);
      for (let i = 0; i < 4; i += 1) rect(ctx, 5, x + 2, y + 3 + i * 3, 8, 1);
    },
    pass(ctx, p) {
      rect(ctx, 4, 128, 30, 4, 44);
      rect(ctx, 4, 150, 30, 4, 44);
      rect(ctx, 2, 128, 28, 26, 3);
      const robotX = 104 + Math.round(Math.max(0, p - 0.4) * 60);
      sprite(ctx, ROBOT, robotX, GROUND_Y - 14);
      const cardX = 84;
      rect(ctx, 7, cardX, 40, 16, 20);
      rect(ctx, 12, cardX + 2, 42, 12, 4);
      for (let i = 0; i < 3; i += 1) rect(ctx, 5, cardX + 2, 49 + i * 3, 12, 1);
      if (p > 0.2) {
        rect(ctx, 8, cardX + 6, 50, 9, 8);
        rect(ctx, 7, cardX + 8, 52, 5, 4);
      }
      if (p > 0.1) {
        for (let i = 0; i < 5; i += 1) rect(ctx, 9, 60 + i * 3, 30 + ((i * 5) % 3), 2, 6);
        rect(ctx, 8, 60, 38, 16, 2);
      }
    },
    bell(ctx, p, t) {
      rect(ctx, 4, 110, 26, 20, 2);
      const swing = p > 0 ? Math.round(Math.sin(t / 60) * 3 * (1 - p)) : 0;
      rect(ctx, 10, 114 + swing, 28, 12, 12);
      rect(ctx, 9, 112 + swing, 38, 16, 3);
      rect(ctx, 0, 119 + swing, 41, 2, 2);
      sprite(ctx, ROBOT, 132, GROUND_Y - 14);
      if (p > 0.4) {
        rect(ctx, 7, 70, 30, 22, 12);
        rect(ctx, 11, 74, 34, 3, 3);
        rect(ctx, 11, 77, 36, 2, 2);
        rect(ctx, 11, 79, 34, 5, 2);
      }
    },
    plug(ctx, p) {
      sprite(ctx, ROBOT, 136, GROUND_Y - 14);
      rect(ctx, 1, 88, 44, 26, 20);
      rect(ctx, 0, 90, 46, 22, 14);
      rect(ctx, 11, 92, 48, 2, 2);
      const reach = Math.round(Math.min(1, p * 1.4) * 22);
      rect(ctx, 5, 114, 60, 2 + reach, 2);
      if (p > 0.7) {
        rect(ctx, 11, 92, 52, 14, 1);
        rect(ctx, 11, 92, 55, 10, 1);
      }
    },
    hourglass(ctx, p) {
      const flip = p > 0.3;
      rect(ctx, 4, 106, 30, 20, 3);
      rect(ctx, 4, 106, 69, 20, 3);
      for (let i = 0; i < 18; i += 1) {
        const w = Math.max(2, 16 - Math.abs(18 - i * 2));
        rect(ctx, 6, 116 - w / 2, 33 + i * 2, w, 2);
      }
      const top = flip ? 1 - p : 0.2;
      rect(ctx, 10, 110, 34, 12, Math.round(12 * top));
      rect(ctx, 10, 110, 68 - Math.round(12 * (1 - top)), 12, Math.round(12 * (1 - top)));
      rect(ctx, 10, 115, 50, 2, 16);
    },
    bars(ctx, p) {
      sprite(ctx, ROBOT, 110, GROUND_Y - 14);
      const drop = Math.round(Math.min(1, p * 1.5) * 44);
      for (let i = 0; i < 5; i += 1) rect(ctx, 5, 100 + i * 8, 30, 2, drop);
      rect(ctx, 5, 98, 28, 36, 3);
      if (p > 0.7) rect(ctx, 8, 104, 22, 28, 3);
    },
    lens(ctx, p, t) {
      sprite(ctx, ROBOT, 116, GROUND_Y - 14);
      const x = 96 + Math.round(Math.sin(t / 300) * (p > 0 ? 12 : 4));
      rect(ctx, 12, x, 40, 16, 16);
      rect(ctx, 7, x + 2, 42, 12, 12);
      rect(ctx, 4, x + 14, 54, 8, 3);
      if (p > 0.4) {
        rect(ctx, 11, x + 4, 45, 8, 1);
        rect(ctx, 11, x + 4, 48, 6, 1);
      }
    },
    scroll(ctx, p) {
      const length = 8 + Math.round(p * 34);
      rect(ctx, 4, 100, 34, 30, 4);
      rect(ctx, 7, 102, 38, 26, length);
      for (let i = 0; i < Math.floor(length / 5); i += 1) rect(ctx, i % 3 === 2 ? 8 : 5, 104, 41 + i * 5, 18 - (i % 2) * 6, 1);
      rect(ctx, 4, 100, 38 + length, 30, 4);
    },
    books(ctx, p) {
      rect(ctx, 4, 94, 34, 44, 40);
      for (let row = 0; row < 3; row += 1) {
        rect(ctx, 0, 96, 46 + row * 12, 40, 1);
        for (let i = 0; i < 9; i += 1) rect(ctx, [8, 12, 11, 10, 14, 9, 13, 7, 2][(i + row) % 9], 97 + i * 4, 36 + row * 12, 3, 10);
      }
      if (p > 0) {
        const lift = Math.round(Math.sin(Math.min(1, p) * Math.PI) * 14);
        rect(ctx, 10, 113, 48 - lift, 3, 10);
        if (p > 0.5) rect(ctx, 7, 118, 44 - lift, 18, 6);
        if (p > 0.5) rect(ctx, 0, 120, 46 - lift, 14, 1);
      }
    },
    cloud(ctx, p) {
      rect(ctx, 7, 96, 30, 36, 12);
      rect(ctx, 7, 104, 24, 20, 22);
      rect(ctx, 5, 96, 44, 36, 1);
      if (p > 0.3) {
        rect(ctx, 10, 110, 52, 6, 6);
        rect(ctx, 10, 115, 54, 8, 2);
        rect(ctx, 10, 121, 54, 2, 4);
      }
    },
    cloudSync(ctx, p, t) {
      PROPS.cloud(ctx, 0, t);
      const phase = p > 0 ? (t / 400) % 1 : 0;
      rect(ctx, 11, 104, 58 - Math.round(phase * 10), 2, 8);
      rect(ctx, 12, 124, 48 + Math.round(phase * 10), 2, 8);
      rect(ctx, 5, 96, 66, 36, 8);
      rect(ctx, 0, 98, 68, 32, 4);
    },
  };

  // -------------------------------- screen --------------------------------
  const view = {
    root: null,
    mode: 'map',
    level: null,
    raf: 0,
    // map
    mapCanvas: null,
    mapBase: null,
    walker: { x: 0, y: 0, path: [], facing: false, onArrive: null, frame: 0 },
    // level
    sceneCanvas: null,
    scene: { state: 'idle', since: 0 },
    hints: 0,
    tries: 0,
    confetti: [],
  };

  function hideAll() {
    cancelAnimationFrame(view.raf);
    view.raf = 0;
  }

  function loop(now) {
    view.raf = requestAnimationFrame(loop);
    if (view.mode === 'map') drawMap(now);
    if (view.mode === 'level') drawScene(now);
  }

  function startLoop() {
    if (!view.raf) view.raf = requestAnimationFrame(loop);
  }

  // ---- map screen ----

  function renderMap() {
    view.mode = 'map';
    const root = view.root;
    root.textContent = '';
    const screen = el('div', 'q-screen q-map');

    const hud = el('div', 'q-hud');
    hud.appendChild(el('span', 'q-badge', '★ ' + totalStars() + ' / ' + LEVELS.length * 3));
    hud.appendChild(el('span', 'q-badge q-badge-soft', '已通关 ' + clearedCount() + ' / ' + LEVELS.length));
    const tools = el('div', 'q-tools');
    tools.appendChild(button('q-btn q-btn-ghost', '? 玩法', () => showIntro(true)));
    tools.appendChild(button('q-btn q-btn-ghost', save.sound ? '♪ 音效开' : '♪ 音效关', (event) => {
      save.sound = !save.sound;
      persist();
      event.target.textContent = save.sound ? '♪ 音效开' : '♪ 音效关';
      Sound.click();
    }));
    hud.appendChild(tools);
    screen.appendChild(hud);

    const stage = el('div', 'q-stage');
    const canvas = el('canvas', 'q-canvas');
    canvas.width = MAP_W;
    canvas.height = MAP_H;
    canvas.setAttribute('role', 'img');
    canvas.setAttribute('aria-label', '像素世界地图：7 个区域由一条小路连起 ' + LEVELS.length + ' 个关卡');
    stage.appendChild(canvas);
    view.mapCanvas = canvas;
    if (!view.mapBase) {
      view.mapBase = document.createElement('canvas');
      view.mapBase.width = MAP_W;
      view.mapBase.height = MAP_H;
      paintMap(view.mapBase.getContext('2d'));
    }

    for (const group of GROUPS) {
      const centre = regionCentre(group.id);
      if (!centre) continue;
      const offset = LANDMARK_OFFSET[group.id] || [0, -24];
      const tag = el('span', 'q-region', (REGIONS[group.id] || {}).place || group.label);
      tag.style.left = ((centre[0] + offset[0] + 12) / MAP_W) * 100 + '%';
      tag.style.top = ((centre[1] + offset[1] - 4) / MAP_H) * 100 + '%';
      stage.appendChild(tag);
    }

    const next = recommended();
    for (const level of LEVELS) {
      const [x, y] = NODES[level.index];
      const stars = starsOf(level);
      const node = button('q-node', '', () => travel(level));
      node.style.left = (x / MAP_W) * 100 + '%';
      node.style.top = (y / MAP_H) * 100 + '%';
      if (stars) node.classList.add('is-done');
      if (next && next.index === level.index) node.classList.add('is-next');
      node.setAttribute(
        'aria-label',
        '第 ' + level.number + ' 关：' + level.drill.title + (stars ? '，已通关 ' + stars + ' 星' : '，未通关'),
      );
      node.title = String(level.number).padStart(2, '0') + ' ' + level.drill.title;
      node.appendChild(el('span', 'q-node-num', String(level.number)));
      stage.appendChild(node);
    }
    const wrap = el('div', 'q-stage-wrap');
    wrap.appendChild(stage);
    screen.appendChild(wrap);

    const actions = el('div', 'q-actions');
    if (next) {
      actions.appendChild(
        button('q-btn q-btn-go', '继续：第 ' + next.number + ' 关 · ' + next.drill.title, () => travel(next)),
      );
    } else {
      actions.appendChild(el('p', 'q-cleared', '全部 ' + LEVELS.length + ' 关都通过了。补满星星，或者去装一个真的 Monica 试试。'));
    }
    screen.appendChild(actions);

    screen.appendChild(renderList());
    const reset = button('q-link', '清空闯关进度', () => {
      if (!window.confirm('清空所有关卡的星星和位置？')) return;
      save.stars = {};
      save.at = 0;
      persist();
      try {
        localStorage.removeItem(OLD_STORE);
      } catch (error) {
        /* ignore */
      }
      view.walker.path = [];
      renderMap();
    });
    screen.appendChild(reset);

    root.appendChild(screen);
    const [sx, sy] = NODES[save.at] || NODES[0];
    if (!view.walker.path.length) {
      view.walker.x = sx;
      view.walker.y = sy;
    }
    startLoop();
    // One dialog at a time: someone who has cleared everything gets the finale,
    // not the welcome.
    if (!next && !view.celebrated) {
      view.celebrated = true;
      save.introSeen = true;
      persist();
      showFinale();
    } else if (!save.introSeen) {
      showIntro(false);
    }
  }

  function renderList() {
    const list = el('div', 'q-list');
    for (const group of GROUPS) {
      const levels = LEVELS.filter((level) => level.region === group.id);
      if (!levels.length) continue;
      const block = el('section', 'q-list-group');
      const done = levels.filter((level) => starsOf(level) > 0).length;
      block.appendChild(el('h3', 'q-list-title', regionLabel(group.id) + '  ' + done + '/' + levels.length));
      for (const level of levels) {
        const item = button('q-list-item', '', () => travel(level));
        item.appendChild(el('span', 'q-list-num', String(level.number).padStart(2, '0')));
        item.appendChild(el('span', 'q-list-name', level.drill.title));
        item.appendChild(el('span', 'q-list-stars' + (starsOf(level) ? ' is-done' : ''), starText(starsOf(level))));
        block.appendChild(item);
      }
      list.appendChild(block);
    }
    return list;
  }

  // Monica walks node by node along the road, then the level opens.
  function travel(level) {
    Sound.click();
    const from = save.at;
    const to = level.index;
    const steps = [];
    const dir = to >= from ? 1 : -1;
    for (let i = from; i !== to; i += dir) steps.push(NODES[i + dir]);
    save.at = to;
    persist();
    if (REDUCED || !steps.length || view.mode !== 'map') {
      openLevel(level);
      return;
    }
    // Long trips are shortened to the last few nodes so a click never feels slow.
    const path = steps.slice(-4);
    if (steps.length > 4) {
      const jump = NODES[to - dir * 4] || NODES[from];
      view.walker.x = jump[0];
      view.walker.y = jump[1];
    }
    view.walker.path = path;
    view.walker.onArrive = () => openLevel(level);
  }

  function drawMap(now) {
    const canvas = view.mapCanvas;
    if (!canvas || !canvas.isConnected) return;
    const ctx = canvas.getContext('2d');
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(view.mapBase, 0, 0);
    // Water glints.
    for (let i = 0; i < 6; i += 1) {
      const phase = (now / 700 + i) % 6;
      if (phase < 1) rect(ctx, 7, 132 + i * 6, 106 + (i % 3) * 5, 2, 1);
    }
    const next = recommended();
    for (const level of LEVELS) {
      const [x, y] = NODES[level.index];
      const stars = starsOf(level);
      rect(ctx, 0, x - 4, y - 3, 9, 7);
      rect(ctx, stars ? 10 : 6, x - 3, y - 2, 7, 5);
      rect(ctx, stars ? 9 : 5, x - 3, y + 2, 7, 1);
      if (next && next.index === level.index) {
        const bob = Math.floor(now / 250) % 2;
        rect(ctx, 8, x - 2, y - 12 + bob, 5, 2);
        rect(ctx, 8, x - 1, y - 10 + bob, 3, 1);
        rect(ctx, 8, x, y - 9 + bob, 1, 1);
      }
    }
    // Walker.
    const w = view.walker;
    let moving = false;
    if (w.path.length) {
      const [tx, ty] = w.path[0];
      const dx = tx - w.x;
      const dy = ty - w.y;
      const dist = Math.hypot(dx, dy);
      const speed = 1.6;
      if (dist <= speed) {
        w.x = tx;
        w.y = ty;
        w.path.shift();
        Sound.step();
        if (!w.path.length && w.onArrive) {
          const arrive = w.onArrive;
          w.onArrive = null;
          window.setTimeout(arrive, 180);
        }
      } else {
        w.x += (dx / dist) * speed;
        w.y += (dy / dist) * speed;
        w.facing = dx < 0;
      }
      moving = true;
    }
    const pose = moving ? (Math.floor(now / 120) % 2 ? 'a' : 'b') : 'stand';
    const blink = !moving && Math.floor(now / 100) % 40 === 0;
    rect(ctx, 0, w.x - 5, w.y + 1, 10, 2);
    sprite(ctx, monica(pose, blink), w.x - 6, w.y - 15, w.facing);
  }

  // ---- intro and finale ----

  const INTRO = [
    '你好，我是 Monica。这是一座命令行练习岛：7 个区域，' + LEVELS.length + ' 个关卡，每关让你敲一条真正的 monica 命令。',
    '这里的命令只按真实语法检查，不会执行，也碰不到任何密码或 Token。放心敲，敲错了我会告诉你差在哪。',
    '卡住了就按「提示」：第一次告诉你用哪条命令，第二次告诉你要带哪些参数，第三次直接给答案。不看提示答对得三颗星。',
  ];

  function typewriter(node, text, done) {
    node.textContent = '';
    if (REDUCED) {
      node.textContent = text;
      if (done) done();
      return () => {};
    }
    let i = 0;
    const timer = window.setInterval(() => {
      i += 1;
      node.textContent = text.slice(0, i);
      if (i >= text.length) {
        window.clearInterval(timer);
        if (done) done();
      }
    }, 28);
    return () => {
      window.clearInterval(timer);
      node.textContent = text;
      if (done) done();
    };
  }

  function showIntro(replay) {
    const stage = view.root.querySelector('.q-stage-wrap');
    if (!stage || stage.querySelector('.q-dialog')) return;
    let page = 0;
    const box = el('div', 'q-dialog');
    box.setAttribute('role', 'dialog');
    box.setAttribute('aria-label', 'Monica 的说明');
    const face = el('canvas', 'q-face');
    face.width = 12;
    face.height = 9;
    sprite(face.getContext('2d'), MONICA_HEAD, 0, 0);
    const text = el('p', 'q-dialog-text');
    const foot = el('div', 'q-dialog-foot');
    const counter = el('span', 'q-dialog-count');
    const skip = button('q-btn q-btn-ghost', '跳过', close);
    const more = button('q-btn q-btn-go', '下一页 ▶', advance);
    foot.appendChild(counter);
    foot.appendChild(skip);
    foot.appendChild(more);
    const head = el('div', 'q-dialog-head');
    head.appendChild(face);
    head.appendChild(el('strong', null, 'Monica'));
    box.appendChild(head);
    box.appendChild(text);
    box.appendChild(foot);
    stage.appendChild(box);
    let finish = show();

    function show() {
      counter.textContent = page + 1 + ' / ' + INTRO.length;
      more.textContent = page === INTRO.length - 1 ? '出发 ▶' : '下一页 ▶';
      return typewriter(text, INTRO[page]);
    }
    function advance() {
      Sound.click();
      if (text.textContent !== INTRO[page]) {
        finish();
        return;
      }
      page += 1;
      if (page >= INTRO.length) {
        close();
        return;
      }
      finish = show();
    }
    function close() {
      finish();
      box.remove();
      save.introSeen = true;
      persist();
    }
    box.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') close();
    });
    more.focus();
    if (replay) Sound.hint();
  }

  function showFinale() {
    const stage = view.root.querySelector('.q-stage-wrap');
    if (!stage) return;
    const box = el('div', 'q-dialog q-finale');
    box.appendChild(el('strong', 'q-finale-title', '全部通关'));
    box.appendChild(
      el('p', 'q-dialog-text', '★ ' + totalStars() + ' / ' + LEVELS.length * 3 + '。你已经敲过建库、存连接、开授权、同步的每一步。'),
    );
    const foot = el('div', 'q-dialog-foot');
    const install = el('a', 'q-btn q-btn-go', '去装一个真的');
    install.href = 'https://github.com/Monica-Pass/Monica-cli#readme';
    foot.appendChild(install);
    foot.appendChild(button('q-btn q-btn-ghost', '补星星', () => box.remove()));
    box.appendChild(foot);
    stage.appendChild(box);
    Sound.good();
  }

  // ---- level screen ----

  function openLevel(level) {
    view.mode = 'level';
    view.level = level;
    view.hints = 0;
    view.tries = 0;
    view.scene = { state: 'idle', since: performance.now() };
    view.confetti = [];
    if (location.hash !== '#quest/' + level.drill.id) {
      history.replaceState(null, '', '#quest/' + level.drill.id);
    }
    const drill = level.drill;
    const root = view.root;
    root.textContent = '';
    const screen = el('div', 'q-screen q-level');

    const bar = el('div', 'q-levelbar');
    bar.appendChild(button('q-btn q-btn-ghost', '← 地图', backToMap));
    bar.appendChild(
      el('span', 'q-level-name', '第 ' + String(level.number).padStart(2, '0') + ' 关 · ' + regionLabel(level.region)),
    );
    bar.appendChild(el('span', 'q-level-stars' + (starsOf(level) ? ' is-done' : ''), starText(starsOf(level))));
    screen.appendChild(bar);

    const grid = el('div', 'q-level-grid');
    const sceneWrap = el('div', 'q-scene-wrap');
    const scene = el('canvas', 'q-scene');
    scene.width = SCENE_W;
    scene.height = SCENE_H;
    scene.setAttribute('role', 'img');
    scene.setAttribute('aria-label', '这一关的像素场景');
    sceneWrap.appendChild(scene);
    view.sceneCanvas = scene;
    grid.appendChild(sceneWrap);

    const task = el('div', 'q-task');
    task.appendChild(el('h2', 'q-task-title', drill.title));
    task.appendChild(el('p', 'q-task-prompt', drill.prompt));
    const field = el('label', 'q-input');
    field.appendChild(el('span', 'q-input-caret', '$'));
    const input = el('input');
    input.type = 'text';
    input.spellcheck = false;
    input.autocomplete = 'off';
    input.setAttribute('autocapitalize', 'off');
    input.placeholder = 'monica …';
    input.setAttribute('aria-label', '你的命令');
    field.appendChild(input);
    task.appendChild(field);

    const actions = el('div', 'q-task-actions');
    actions.appendChild(button('q-btn q-btn-go', '检查 ⏎', () => submit(input)));
    const hint = button('q-btn', '提示 1/3', () => giveHint(hint, input));
    actions.appendChild(hint);
    task.appendChild(actions);
    task.appendChild(el('div', 'q-hints'));
    task.appendChild(el('div', 'q-verdict'));
    grid.appendChild(task);
    screen.appendChild(grid);
    screen.appendChild(el('div', 'q-after'));
    root.appendChild(screen);

    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        submit(input);
      }
    });
    input.focus({ preventScroll: true });
    root.scrollIntoView({ block: 'start', behavior: REDUCED ? 'auto' : 'smooth' });
    startLoop();
  }

  function backToMap() {
    Sound.click();
    history.replaceState(null, '', '#quest');
    view.walker.path = [];
    renderMap();
  }

  function hintLines(level, depth) {
    const drill = level.drill;
    const cmd = CMD.get(drill.expect.key);
    const lines = [];
    if (depth >= 1) {
      lines.push([
        '用哪条命令',
        'monica ' + drill.expect.key + (cmd && cmd.summaryZh ? ' — ' + cmd.summaryZh : ''),
      ]);
    }
    if (depth >= 2) {
      const flags = (drill.expect.flags || []).map((flag) => {
        const arg = cmd && cmd.args.find((item) => '--' + item.long === flag || item.long === flag.replace(/^--/, ''));
        return arg && arg.short ? '-' + arg.short + ' / ' + flag : flag;
      });
      const parts = [];
      parts.push(flags.length ? '选项：' + flags.join('，') : '不需要额外的选项');
      if (typeof drill.expect.positionals === 'number' && drill.expect.positionals > 0) {
        parts.push('位置参数 ' + drill.expect.positionals + ' 个');
      }
      const avoid = (drill.expect.forbid || []).concat(drill.expect.mustAbsent || []);
      if (avoid.length) parts.push('别带 ' + avoid.join('、'));
      lines.push(['要带哪些参数', parts.join('；')]);
    }
    if (depth >= 3) lines.push(['答案', drill.answer]);
    return lines;
  }

  function giveHint(hintButton, input) {
    if (view.hints >= 3) return;
    view.hints += 1;
    Sound.hint();
    const host = view.root.querySelector('.q-hints');
    host.textContent = '';
    for (const [label, text] of hintLines(view.level, view.hints)) {
      const row = el('div', 'q-hint');
      row.appendChild(el('span', 'q-hint-label', label));
      row.appendChild(el('code', 'q-hint-text', text));
      host.appendChild(row);
    }
    if (view.hints >= 3) {
      hintButton.disabled = true;
      hintButton.textContent = '已给出答案';
      input.value = view.level.drill.answer;
    } else {
      hintButton.textContent = '提示 ' + (view.hints + 1) + '/3';
    }
    input.focus({ preventScroll: true });
  }

  function earned() {
    if (view.hints >= 3) return 1;
    if (view.hints >= 1) return 2;
    return 3;
  }

  function submit(input) {
    const level = view.level;
    const line = (input.value || '').trim();
    const verdict = view.root.querySelector('.q-verdict');
    verdict.textContent = '';
    if (!line) {
      verdict.className = 'q-verdict is-bad';
      verdict.appendChild(el('p', null, '先敲一条命令，比如 monica …'));
      return;
    }
    const graded = GRAMMAR.gradeDrill(engine, line, level.drill);
    input.classList.toggle('is-bad', !graded.ok);
    if (!graded.ok) {
      view.tries += 1;
      verdict.className = 'q-verdict is-bad';
      verdict.appendChild(el('strong', null, '还差一点 · 第 ' + view.tries + ' 次'));
      const list = el('ul');
      for (const problem of graded.problems) list.appendChild(el('li', null, problem));
      verdict.appendChild(list);
      if (view.tries >= 2 && view.hints < 3) {
        verdict.appendChild(el('p', 'q-nudge', '卡住了？按「提示」，一次只揭开一层。'));
      }
      view.scene = { state: 'wrong', since: performance.now() };
      Sound.bad();
      return;
    }
    const stars = earned();
    const before = starsOf(level);
    save.stars[level.drill.id] = Math.max(before, stars);
    persist();
    verdict.className = 'q-verdict is-ok';
    verdict.appendChild(el('strong', null, '通过 ' + starText(stars) + (before > stars ? '（保留之前的 ' + before + ' 星）' : '')));
    input.disabled = true;
    for (const node of view.root.querySelectorAll('.q-task-actions button')) node.disabled = true;
    const starsNode = view.root.querySelector('.q-level-stars');
    starsNode.textContent = starText(starsOf(level));
    starsNode.classList.add('is-done');
    view.scene = { state: 'win', since: performance.now() };
    spawnConfetti();
    Sound.good();
    window.setTimeout(() => showAfter(level, line), REDUCED ? 0 : 900);
  }

  function testedExample(level) {
    const cmd = CMD.get(level.drill.expect.key);
    if (!cmd) return null;
    return cmd.examples.find((example) => example.tested && example.out && !example.teachesError) || null;
  }

  function showAfter(level, line) {
    if (view.level !== level) return;
    const host = view.root.querySelector('.q-after');
    host.textContent = '';
    const example = testedExample(level);
    const term = el('div', 'q-terminal');
    const head = el('div', 'q-terminal-head');
    head.appendChild(el('span', null, example ? '实测输出示例' : '这条命令没有采集输出'));
    head.appendChild(el('span', 'q-terminal-note', '合成凭据 · 临时库 · 0.5.0 release'));
    term.appendChild(head);
    const pre = el('pre', 'q-terminal-body');
    term.appendChild(pre);
    host.appendChild(term);
    const lines = example
      ? ['$ ' + example.cmd].concat(example.out.split('\n'))
      : ['$ ' + line, '（' + ((CMD.get(level.drill.expect.key) || {}).examples || []).map((e) => e.reason).filter(Boolean)[0] + '）'];
    let shown = 0;
    let timer = 0;
    const flush = () => {
      window.clearInterval(timer);
      pre.textContent = lines.join('\n');
    };
    if (REDUCED) {
      flush();
    } else {
      timer = window.setInterval(() => {
        shown += 1;
        pre.textContent = lines.slice(0, shown).join('\n');
        if (shown >= lines.length) window.clearInterval(timer);
      }, 70);
      term.addEventListener('click', flush);
    }

    const why = el('div', 'q-why');
    why.appendChild(el('strong', null, '为什么这样写'));
    why.appendChild(el('p', null, level.drill.why));
    if (line !== level.drill.answer) {
      why.appendChild(el('p', 'q-alt', '参考写法：' + level.drill.answer));
    }
    host.appendChild(why);

    const nav = el('div', 'q-after-nav');
    const next = LEVELS.slice(level.index + 1).find((item) => starsOf(item) === 0) || recommended();
    if (next) {
      nav.appendChild(button('q-btn q-btn-go', '下一关：' + next.drill.title + ' ▶', () => {
        save.at = level.index;
        view.walker.x = NODES[level.index][0];
        view.walker.y = NODES[level.index][1];
        history.replaceState(null, '', '#quest');
        renderMap();
        travel(next);
      }));
    }
    nav.appendChild(button('q-btn', '回地图', backToMap));
    const detail = el('a', 'q-btn q-btn-ghost', '看 ' + level.drill.expect.key + ' 的详情');
    detail.href = '#detail/' + encodeURIComponent(level.drill.expect.key);
    nav.appendChild(detail);
    host.appendChild(nav);
    const focus = nav.querySelector('button');
    if (focus) focus.focus({ preventScroll: true });
    host.scrollIntoView({ block: 'nearest', behavior: REDUCED ? 'auto' : 'smooth' });
  }

  function spawnConfetti() {
    if (REDUCED) return;
    const random = rng(Date.now());
    view.confetti = [];
    for (let i = 0; i < 60; i += 1) {
      view.confetti.push({
        x: 20 + random() * 120,
        y: -random() * 40,
        vy: 0.4 + random() * 0.8,
        vx: (random() - 0.5) * 0.6,
        c: [8, 9, 10, 11, 12, 14][Math.floor(random() * 6)],
      });
    }
  }

  function drawScene(now) {
    const canvas = view.sceneCanvas;
    if (!canvas || !canvas.isConnected) return;
    const level = view.level;
    const ctx = canvas.getContext('2d');
    ctx.imageSmoothingEnabled = false;
    paintBackdrop(ctx, level.region, now);
    const kind = SCENES[level.drill.id] || REGION_PROP[level.region] || 'sign';
    const elapsed = now - view.scene.since;
    const p = view.scene.state === 'win' ? (REDUCED ? 1 : Math.min(1, elapsed / 1400)) : 0;
    (PROPS[kind] || PROPS.sign)(ctx, p, now);

    // Monica stands left of the prop; she shakes on a wrong answer and cheers on a win.
    let x = 40;
    let pose = 'stand';
    if (view.scene.state === 'wrong' && elapsed < 500 && !REDUCED) {
      x += Math.round(Math.sin(elapsed / 30) * 2);
    }
    if (view.scene.state === 'win') {
      pose = Math.floor(now / 220) % 2 ? 'cheer' : 'stand';
    }
    const hop = view.scene.state === 'win' && !REDUCED ? Math.abs(Math.round(Math.sin(now / 160) * 3)) : 0;
    const breathe = view.scene.state === 'idle' ? Math.floor(now / 600) % 2 : 0;
    rect(ctx, 0, x - 1, GROUND_Y - 1, 14, 2);
    sprite(ctx, monica(pose, Math.floor(now / 100) % 45 === 0), x, GROUND_Y - 16 - hop + breathe);
    if (view.scene.state === 'wrong' && elapsed < 1600) {
      rect(ctx, 7, x + 12, GROUND_Y - 30, 9, 10);
      rect(ctx, 8, x + 15, GROUND_Y - 28, 3, 1);
      rect(ctx, 8, x + 17, GROUND_Y - 27, 1, 2);
      rect(ctx, 8, x + 16, GROUND_Y - 25, 1, 1);
      rect(ctx, 8, x + 16, GROUND_Y - 23, 1, 1);
    }
    if (view.scene.state === 'idle') {
      // A blinking cursor above her head: she is waiting for your command.
      if (Math.floor(now / 450) % 2) rect(ctx, 10, x + 4, GROUND_Y - 22, 4, 1);
    }
    for (const bit of view.confetti) {
      bit.y += bit.vy;
      bit.x += bit.vx;
      if (bit.y < SCENE_H) rect(ctx, bit.c, bit.x, bit.y, 2, 2);
    }
  }

  // --------------------------------- api ----------------------------------
  // site.js owns the views and the URL; it calls these when #quest is shown.
  window.MONICA_QUEST = {
    mount(root) {
      view.root = root;
    },
    show(arg) {
      if (!view.root) return;
      const level = arg ? LEVEL_BY_ID.get(arg) : null;
      if (level) {
        save.at = level.index;
        persist();
        openLevel(level);
      } else {
        renderMap();
      }
    },
    hide: hideAll,
    levelIds: LEVELS.map((level) => level.drill.id),
  };
})();
