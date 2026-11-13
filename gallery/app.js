// Offline gallery for br1conv output. Data comes from manifest.js (window.BR1).
// Models are loaded from models/*.glb.js wrappers (base64) so file:// works.
(function () {
  const M = window.BR1 || {};
  const $ = (sel, el = document) => el.querySelector(sel);
  const h = (tag, attrs = {}, ...kids) => {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
      else if (v !== undefined && v !== null && v !== false) el.setAttribute(k, v === true ? '' : v);
    }
    for (const k of kids.flat()) if (k !== null && k !== undefined) el.append(k.nodeType ? k : String(k));
    return el;
  };
  const fmtTime = (s) => (s >= 60 ? `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, '0')}` : `${(+s).toFixed(1)}s`);
  const sfxCount = (M.sfx || []).reduce((n, b) => n + b.items.length, 0);
  // Model kinds by manifest group: "Fighter XXX", "Stages", "VFX".
  function modelsOf(kind) {
    const kindOf = (m) => (m.group === 'Stages' ? 'stages' : m.group === 'VFX' ? 'vfx' : 'characters');
    return (M.models || []).filter((m) => kindOf(m) === kind);
  }

  const tabs = [
    { id: 'overview', label: 'Overview', render: overview },
    { id: 'characters', label: 'Characters', count: modelsOf('characters').length, render: (v) => models(v, 'characters') },
    { id: 'stages', label: 'Stages', count: modelsOf('stages').length, render: (v) => models(v, 'stages') },
    { id: 'vfx', label: 'VFX', count: (M.vfx || []).length + modelsOf('vfx').length, render: vfxView },
    { id: 'images', label: 'Images', count: (M.images || []).length, render: images },
    { id: 'textures', label: 'Textures', count: (M.backgrounds || []).length + (M.textures || []).length, render: textures },
    { id: 'music', label: 'Music', count: (M.music || []).length, render: music },
    { id: 'sfx', label: 'SFX', count: sfxCount, render: sfx },
    { id: 'video', label: 'Video', count: (M.videos || []).length, render: videos },
  ];

  let disposeView = null;
  function show(id) {
    if (disposeView) { disposeView(); disposeView = null; }
    const tab = tabs.find((t) => t.id === (id === 'models' ? 'characters' : id)) || tabs[0];
    for (const b of document.querySelectorAll('#tabs button')) b.classList.toggle('on', b.dataset.id === tab.id);
    const view = $('#view');
    view.replaceChildren();
    disposeView = tab.render(view) || null;
    try { localStorage.setItem('br1-tab', tab.id); } catch (e) { /* storage unavailable */ }
    history.replaceState(null, '', '#' + tab.id);
  }

  for (const t of tabs) {
    $('#tabs').append(h('button', { 'data-id': t.id, onclick: () => show(t.id) }, t.label, t.count !== undefined ? h('span', { class: 'n' }, t.count) : null));
  }

  // Sortable table: cols = [{key, label, num?, render?}]
  function table(rows, cols) {
    let sortKey = null, asc = true;
    const tbody = h('tbody');
    const head = h('tr');
    const draw = () => {
      const r = [...rows];
      if (sortKey) r.sort((a, b) => { const x = a[sortKey], y = b[sortKey]; return (x > y ? 1 : x < y ? -1 : 0) * (asc ? 1 : -1); });
      tbody.replaceChildren(...r.map((row) => h('tr', {}, cols.map((c) => h('td', { class: c.num ? 'num' : null }, c.render ? c.render(row) : row[c.key] ?? '')))));
    };
    for (const c of cols) {
      const th = h('th', { onclick: () => {
        if (!c.key) return;
        asc = sortKey === c.key ? !asc : true; sortKey = c.key;
        for (const t of head.children) t.classList.remove('sorted', 'asc');
        th.classList.add('sorted'); th.classList.toggle('asc', asc); draw();
      } }, c.label);
      head.append(th);
    }
    draw();
    return h('div', { class: 'table-wrap' }, h('table', {}, h('thead', {}, head), tbody));
  }

  function overview(view) {
    const stat = (n, label) => h('div', { class: 'stat' }, h('b', {}, n), h('span', {}, label));
    view.append(
      h('h2', {}, 'Export summary'),
      h('div', { class: 'stats' },
        stat(modelsOf('characters').length, 'character models'), stat(modelsOf('stages').length, 'stages'), stat((M.vfx || []).length, 'effect animations'), stat((M.images || []).length, 'images'),
        stat((M.textures || []).length, 'textures'), stat((M.backgrounds || []).length, 'stage panoramas'), stat((M.music || []).length, 'music tracks'),
        stat(sfxCount, 'sound effects'), stat((M.videos || []).length, 'videos')),
      h('h2', {}, 'Formats'),
      table([
        { kind: '3D characters / stages', src: 'XX.ANM, XX20.DAT, …', out: 'glTF 2.0 (.glb): mesh, skeleton, animations, embedded PNG' },
        { kind: 'Images', src: 'ART*/*.TIM', out: 'PNG (RGBA, 0x0000 = transparent)' },
        { kind: 'Textures', src: 'XX1x.DAT, CMN/*.DAT (compressed TIM)', out: 'PNG per CLUT' },
        { kind: 'Stage backgrounds', src: 'XX00.DAT tile map (0x23) + tile defs (0x22) + 8bpp TIM', out: 'PNG panorama + cylinder in stage .glb' },
        { kind: 'Music', src: 'MOVIE/BGM*.XA, CD-DA track 2', out: 'FLAC' },
        { kind: 'Sound effects', src: '*.SND (VAB banks)', out: 'WAV (per VAG sample)' },
        { kind: 'Video', src: 'MOVIE/*.STR (MDEC)', out: 'MP4 (H.264 + AAC)' },
        { kind: 'VFX', src: 'EXE particle tables + CMN/EFFECT.DAT', out: 'animated sprite sheets (60 ticks/s) + vfx.json definitions' },
        { kind: 'Shaders', src: 'PS1 GPU render state', out: 'glTF materials + PS1 look shaders (three.js, Blender, Godot)' },
      ], [{ key: 'kind', label: 'Resource' }, { key: 'src', label: 'Source on disc' }, { key: 'out', label: 'Exported as' }]),
    );
  }

  function lightbox(src, caption) {
    const lb = $('#lightbox');
    $('img', lb).src = src; $('p', lb).textContent = caption; lb.hidden = false;
  }
  $('#lightbox').addEventListener('click', () => { $('#lightbox').hidden = true; });

  function imageGrid(view, list, title) {
    const groups = [...new Set(list.map((i) => i.group || ''))];
    const sel = h('select', {}, h('option', { value: '' }, 'All groups'), groups.map((g) => h('option', { value: g }, g || '(none)')));
    const q = h('input', { type: 'search', placeholder: 'Filter by name…' });
    const grid = h('div', { class: 'grid' });
    const draw = () => {
      const f = q.value.toLowerCase();
      grid.replaceChildren(...list.filter((i) => (!sel.value || i.group === sel.value) && i.name.toLowerCase().includes(f)).map((i) =>
        h('div', { class: 'card' },
          h('img', { src: i.path, loading: 'lazy', alt: i.name, onclick: () => lightbox(i.path, `${i.name} · ${i.width}×${i.height} · ${i.bpp}bpp · ${i.source}`) }),
          h('div', { class: 'meta' }, h('b', {}, i.name), h('span', { class: 'muted' }, `${i.width}×${i.height} ${i.bpp}bpp`)))));
    };
    sel.onchange = draw; q.oninput = draw;
    view.append(h('div', { class: 'toolbar' }, q, groups.length > 1 ? sel : null, h('span', { class: 'muted' }, title)), grid);
    draw();
  }

  function images(view) { imageGrid(view, M.images || [], 'Full-screen art (TIM → PNG)'); }

  function textures(view) {
    const list = (M.backgrounds || []).concat(M.textures || []);
    if (!list.length) return void view.append(h('div', { class: 'empty' }, 'No textures exported yet.'));
    imageGrid(view, list, 'Stage panoramas and decompressed game textures');
  }

  function music(view) {
    view.append(table(M.music || [], [
      { key: 'name', label: 'Track' },
      { key: 'source', label: 'Source' },
      { key: 'seconds', label: 'Length', num: true, render: (r) => fmtTime(r.seconds) },
      { key: 'rate', label: 'Rate', num: true, render: (r) => `${r.rate} Hz ${r.channels === 2 ? 'stereo' : 'mono'}` },
      { label: 'Play', render: (r) => h('audio', { controls: true, preload: 'none', src: r.path }) },
      { label: 'File', render: (r) => h('a', { href: r.path, download: true }, 'FLAC') },
    ]));
  }

  function sfx(view) {
    const player = new Audio();
    let playing = null;
    const q = h('input', { type: 'search', placeholder: 'Filter banks…' });
    const mode = { value: 'chips' };
    const body = h('div');
    const draw = () => {
      const banks = (M.sfx || []).filter((b) => b.bank.toLowerCase().includes(q.value.toLowerCase()));
      if (mode.value === 'table') {
        body.replaceChildren(table(banks.flatMap((b) => b.items.map((i) => ({ ...i, bank: b.bank }))), [
          { key: 'bank', label: 'Bank' }, { key: 'name', label: 'Sample' },
          { key: 'seconds', label: 'Length', num: true, render: (r) => `${r.seconds}s` },
          { key: 'rate', label: 'Rate', num: true }, { key: 'loop', label: 'Loop', render: (r) => (r.loop ? 'yes' : '') },
          { label: 'Play', render: (r) => h('audio', { controls: true, preload: 'none', src: r.path }) },
        ]));
        return;
      }
      body.replaceChildren(...banks.map((b) => h('div', { class: 'bank' },
        h('h3', {}, b.bank, ' ', h('span', { class: 'muted' }, `${b.source} · ${b.items.length} samples`)),
        h('div', { class: 'chips' }, b.items.map((i) => {
          const chip = h('button', { class: 'chip', title: `${i.rate} Hz · ${i.seconds}s${i.loop ? ' · loop' : ''}`, onclick: () => {
            if (playing) playing.classList.remove('playing');
            player.src = i.path; player.play(); playing = chip; chip.classList.add('playing');
          } }, i.name.split('_').pop());
          return chip;
        })))));
    };
    player.onended = () => playing && playing.classList.remove('playing');
    const b1 = h('button', { class: 'btn on', onclick: () => { mode.value = 'chips'; b1.classList.add('on'); b2.classList.remove('on'); draw(); } }, 'Pads');
    const b2 = h('button', { class: 'btn', onclick: () => { mode.value = 'table'; b2.classList.add('on'); b1.classList.remove('on'); draw(); } }, 'Table');
    q.oninput = draw;
    view.append(h('div', { class: 'toolbar' }, q, b1, b2, h('span', { class: 'muted' }, 'VAB banks → WAV')), body);
    draw();
    return () => player.pause();
  }

  function videos(view) {
    view.append(h('div', { class: 'grid' }, (M.videos || []).map((v) =>
      h('div', { class: 'card' }, h('video', { controls: true, preload: 'none', poster: v.poster, src: v.path }),
        h('div', { class: 'meta' }, h('b', {}, v.name), h('span', { class: 'muted' }, v.source))))));
  }

  // ---------- VFX: sprite-sheet players (60 ticks/s) + effect model ----------
  function vfxView(view) {
    const items = M.vfx || [];
    const players = [];
    const kinds = [['spark', 'Hit sparks (per preset; _blood = blood option on)'], ['particle', 'Particle types (flipbooks)'], ['motion', 'Moving particles']];
    for (const [kind, title] of kinds) {
      const list = items.filter((i) => i.kind === kind);
      if (!list.length) continue;
      view.append(h('h2', {}, title));
      const grid = h('div', { class: 'grid', style: 'grid-template-columns:repeat(auto-fill,minmax(160px,1fr))' });
      for (const it of list) {
        const bg = it.background || 0;
        const cv = h('canvas', { width: it.cell, height: it.cell, style: `width:100%;aspect-ratio:1;background:rgb(${bg},${bg},${bg});image-rendering:pixelated;display:block` });
        const img = new Image(); img.src = it.path;
        players.push({ it, cv, img });
        grid.append(h('div', { class: 'card' }, cv, h('div', { class: 'meta' }, h('b', {}, it.name), h('span', { class: 'muted' }, `${it.frames}f`)),
          h('div', { class: 'meta muted', style: 'padding-top:0' }, it.note)));
      }
      view.append(grid);
    }
    let disposeModels = null;
    if (modelsOf('vfx').length) {
      view.append(h('h2', {}, 'Effect model (EFFECT.DAT TMD)'));
      const holder = h('div');
      view.append(holder);
      disposeModels = models(holder, 'vfx');
    }
    if (!items.length && !modelsOf('vfx').length) view.append(h('div', { class: 'empty' }, 'No effects exported yet.'));
    const t0 = performance.now();
    let raf = 0;
    const tick = () => {
      raf = requestAnimationFrame(tick);
      const f = Math.floor((performance.now() - t0) / 1000 * 60);
      for (const { it, cv, img } of players) {
        if (!img.complete || !img.naturalWidth) continue;
        const k = f % (it.frames + 20); // short pause between loops
        const ctx = cv.getContext('2d');
        ctx.clearRect(0, 0, it.cell, it.cell);
        if (k < it.frames) ctx.drawImage(img, (k % it.cols) * it.cell, Math.floor(k / it.cols) * it.cell, it.cell, it.cell, 0, 0, it.cell, it.cell);
      }
    };
    tick();
    return () => { cancelAnimationFrame(raf); if (disposeModels) disposeModels(); };
  }

  // ---------- 3D viewer ----------
  window.BR1_GLB = window.BR1_GLB || {};
  function loadGlb(model) {
    return new Promise((resolve, reject) => {
      if (window.BR1_GLB[model.name]) return resolve(window.BR1_GLB[model.name]);
      const s = document.createElement('script');
      s.src = model.js;
      s.onload = () => resolve(window.BR1_GLB[model.name]);
      s.onerror = () => reject(new Error('cannot load ' + model.js));
      document.head.append(s);
    }).then((b64) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)).buffer);
  }

  function models(view, kind) {
    const list = modelsOf(kind);
    if (!list.length) return void view.append(h('div', { class: 'empty' }, 'No models exported yet.'));
    const side = h('div', { class: 'side' });
    const stage = h('div', { class: 'stage' });
    const info = h('div', { class: 'info' }, '');
    const animSel = h('select', {});
    const ps1Btn = h('button', { class: 'btn' + (PS1.enabled ? ' on' : ''), onclick: () => { PS1.enabled = !PS1.enabled; ps1Btn.classList.toggle('on', PS1.enabled); } }, 'PS1 shader');
    const wireBtn = h('button', { class: 'btn', onclick: () => { wire = !wire; wireBtn.classList.toggle('on', wire); setWire(); } }, 'Wireframe');
    const bonesBtn = h('button', { class: 'btn', onclick: () => { if (helper) { helper.visible = !helper.visible; bonesBtn.classList.toggle('on', helper.visible); } } }, 'Bones');
    const playBtn = h('button', { class: 'btn on', onclick: () => { paused = !paused; playBtn.classList.toggle('on', !paused); } }, 'Play');
    const dl = h('a', { class: 'btn', download: true }, 'Download .glb');
    const rigged = kind === 'characters'; // stages and VFX have no skeleton or animations
    const costumeSel = h('select', { title: 'Costume (material variant)' });
    // Root motion off = clips play in place (bone_00 tracks removed).
    let rootMotion = false, sound = true;
    const rootBtn = h('button', { class: 'btn', title: 'Apply the clip\'s root motion (character moves/turns)', onclick: () => { rootMotion = !rootMotion; rootBtn.classList.toggle('on', rootMotion); playClip(+animSel.value); } }, 'Root motion');
    const soundBtn = h('button', { class: 'btn on', title: 'Play the clip\'s sound cues', onclick: () => { sound = !sound; soundBtn.classList.toggle('on', sound); } }, 'Sound');
    const clipInfo = h('div', { class: 'info', style: 'top:auto;bottom:52px;max-width:70%' }, '');
    stage.append(info, clipInfo, h('div', { class: 'hud' }, rigged ? [costumeSel, animSel, playBtn, rootBtn, soundBtn] : [], ps1Btn, wireBtn, rigged ? bonesBtn : null, dl));
    view.append(h('div', { class: 'viewer' }, side, stage));

    const renderer = new THREE.WebGLRenderer({ antialias: false });
    renderer.setPixelRatio(1);
    renderer.outputEncoding = THREE.sRGBEncoding;
    stage.prepend(renderer.domElement);
    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x14111a);
    const grid = new THREE.GridHelper(12, 24, 0x3a3340, 0x241f29);
    scene.add(grid);
    scene.add(new THREE.HemisphereLight(0xffffff, 0x404050, 0.9));
    const sun = new THREE.DirectionalLight(0xffffff, 0.8); sun.position.set(2, 4, 3); scene.add(sun);
    const camera = new THREE.PerspectiveCamera(45, 1, 1, 100000);
    const controls = new THREE.OrbitControls(camera, renderer.domElement);
    const clock = new THREE.Clock();
    let root = null, mixer = null, helper = null, wire = false, paused = false, raf = 0, clips = [];
    let extras = [], action = null, lastT = 0;
    // The .glb JSON chunk: three.js drops animation extras and material variants.
    function glbDoc(buf) {
      try {
        const dv = new DataView(buf), len = dv.getUint32(12, true);
        return JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 20, len)));
      } catch (e) { return {}; }
    }
    // KHR_materials_variants: swap each mesh's texture to the variant's material texture.
    let gltfRef = null, doc = {};
    costumeSel.onchange = async () => {
      const v = +costumeSel.value;
      if (!gltfRef) return;
      const parser = gltfRef.parser;
      const jobs = [];
      root.traverse((o) => {
        if (!o.isMesh) return;
        const a = parser.associations.get(o);
        if (!a || a.meshes === undefined) return;
        const prim = doc.meshes[a.meshes].primitives[a.primitives || 0];
        const map = ((prim.extensions || {}).KHR_materials_variants || {}).mappings || [];
        const hit = map.find((m) => m.variants.includes(v));
        if (!hit) return;
        const texRef = ((doc.materials[hit.material] || {}).pbrMetallicRoughness || {}).baseColorTexture;
        if (!texRef || !o.material.map) return;
        jobs.push(parser.getDependency('texture', texRef.index).then((tex) => {
          tex.magFilter = tex.minFilter = THREE.NearestFilter; tex.generateMipmaps = false;
          o.material.map = tex; o.material.needsUpdate = true;
        }));
      });
      await Promise.all(jobs);
    };

    function setWire() { root && root.traverse((o) => { if (o.isMesh) for (const m of [].concat(o.material)) m.wireframe = wire; }); }
    function resize() {
      const w = stage.clientWidth, hgt = Math.max(stage.clientHeight, 400);
      renderer.setSize(w, hgt, false); camera.aspect = w / hgt; camera.updateProjectionMatrix();
      PS1.uniforms.uSnap.value.set(320, 320 * hgt / w);
    }
    const ro = new ResizeObserver(resize); ro.observe(stage);
    window.BR1_VIEW = { camera, controls, scene, get root() { return root; } }; // console/debug access

    async function open(model, item) {
      for (const el of side.children) el.classList.remove('on');
      item.classList.add('on');
      info.textContent = 'Loading ' + model.name + '…';
      dl.href = model.path;
      if (root) { scene.remove(root); scene.remove(helper); }
      const buf = await loadGlb(model);
      doc = glbDoc(buf);
      extras = (doc.animations || []).map((a) => a.extras || {});
      const gltf = await new Promise((res, rej) => new THREE.GLTFLoader().parse(buf, '', res, rej));
      gltfRef = gltf;
      const variants = ((doc.extensions || {}).KHR_materials_variants || {}).variants || [];
      costumeSel.replaceChildren(...variants.map((v, i) => h('option', { value: i }, v.name)));
      costumeSel.style.display = variants.length ? '' : 'none';
      root = gltf.scene; scene.add(root);
      PS1.apply(root); setWire();
      helper = new THREE.SkeletonHelper(root); helper.visible = bonesBtn.classList.contains('on'); scene.add(helper);
      clips = gltf.animations; mixer = new THREE.AnimationMixer(root);
      animSel.replaceChildren(h('option', { value: -1 }, clips.length ? '(bind pose)' : 'no animations'), ...clips.map((c, i) => h('option', { value: i }, c.name || `anim ${i}`)));
      if (clips.length) { animSel.value = 0; playClip(0); }
      // Frame the core of the model: stages have far-away scenery.
      const box = new THREE.Box3().setFromObject(root);
      const core = new THREE.Box3(new THREE.Vector3(-8, -1, -8), new THREE.Vector3(8, 6, 8));
      if (box.getSize(new THREE.Vector3()).length() > 30) box.intersect(core);
      const size = box.getSize(new THREE.Vector3()).length() || 1, c = box.getCenter(new THREE.Vector3());
      controls.target.copy(c); camera.position.copy(c).add(new THREE.Vector3(0, size * 0.15, size * 1.1)); camera.near = size / 100; camera.far = size * 100; camera.updateProjectionMatrix();
      let tris = 0; root.traverse((o) => { if (o.isMesh) tris += (o.geometry.index ? o.geometry.index.count : o.geometry.attributes.position.count) / 3; });
      info.textContent = `${model.name} · ${Math.round(tris)} tris · ${clips.length} anims${model.note ? ' · ' + model.note : ''}`;
    }
    function playClip(i) {
      mixer.stopAllAction(); mixer.uncacheRoot(root); action = null;
      root.traverse((o) => { if (o.isSkinnedMesh) o.skeleton.pose(); }); // back to rest pose
      const x = extras[i] || {};
      clipInfo.textContent = i >= 0 && clips[i]
        ? [x.forms && x.forms.join('+'), x.loop ? 'loop' : 'one-shot', x.source === 'CMN/CMN.ACT' ? 'shared clip' : null,
           (x.sfx || []).length ? `${x.sfx.length} sound cue(s)` : null, (x.states || []).slice(0, 2).join('; ')].filter(Boolean).join(' · ')
        : '';
      if (!(i >= 0 && clips[i])) return;
      let clip = clips[i];
      if (!rootMotion) clip = new THREE.AnimationClip(clip.name, clip.duration, clip.tracks.filter((t) => !t.name.startsWith('bone_00.')));
      action = mixer.clipAction(clip); action.play(); lastT = 0;
    }
    // Sound cues: play each cue whose time was crossed since the last frame.
    function playCues() {
      if (!action || !sound) return;
      const x = extras[+animSel.value] || {}, t = action.time;
      for (const c of x.sfx || []) {
        if (!c.wav) continue;
        const crossed = t >= lastT ? c.time >= lastT && c.time < t : c.time >= lastT || c.time < t;
        if (crossed) { const a = new Audio(c.wav); a.volume = 0.6; a.play().catch(() => {}); }
      }
      lastT = t;
    }
    animSel.onchange = () => playClip(+animSel.value);

    const groups = {};
    for (const m of list) (groups[m.group || 'Models'] ||= []).push(m);
    for (const [g, ms] of Object.entries(groups)) {
      side.append(h('div', { class: 'muted', style: 'margin:8px 0 4px;font-size:12px' }, g));
      for (const m of ms) { const it = h('div', { class: 'item' }, m.name); it.onclick = () => open(m, it); side.append(it); }
    }
    const loop = () => { raf = requestAnimationFrame(loop); const dt = clock.getDelta(); if (mixer && !paused) { mixer.update(dt); playCues(); } controls.update(); renderer.render(scene, camera); };
    resize(); loop();
    const first = side.querySelector('.item'); if (first) first.click();
    return () => { cancelAnimationFrame(raf); ro.disconnect(); renderer.dispose(); };
  }

  let initial = location.hash.slice(1);
  try { initial = initial || localStorage.getItem('br1-tab') || ''; } catch (e) { /* storage unavailable */ }
  show(initial || 'overview');
  window.addEventListener('hashchange', () => show(location.hash.slice(1)));
})();
