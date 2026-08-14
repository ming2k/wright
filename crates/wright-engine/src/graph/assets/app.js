/* Interactive plan dependency graph for `wright graph --web`.
 *
 * Single-file vanilla JS: no libraries, no build step, no external assets.
 * The graph document is fetched once from /api/graph on load (and again via
 * the refresh button), laid out as a left-to-right layered DAG (Kahn
 * layering + barycenter crossing reduction), and rendered as plain SVG
 * inside a pan/zoom viewport. Filtering hides nodes and their edges in
 * place — the layout never reflows, so the picture stays mentally stable.
 */
(function () {
  'use strict';

  /* ------------------------------------------------------------------ *
   * Constants
   * ------------------------------------------------------------------ */

  var SVG_NS = 'http://www.w3.org/2000/svg';
  var NODE_H = 40; // node box height (two text lines)
  var NODE_PAD_X = 14; // horizontal padding inside a node box
  var NODE_MIN_W = 72;
  var NAME_CHAR_W = 7.2; // estimated px per char on the 12px name line
  var VER_CHAR_W = 5.8; // ... and on the 10px version line
  var LAYER_GAP = 90; // horizontal gap between layers
  var ROW_GAP = 16; // vertical gap between nodes in one layer
  var BARYCENTER_PASSES = 3; // forward/backward crossing-reduction sweeps
  var PARALLEL_SPREAD = 7; // vertical offset between parallel edges
  var MIN_ZOOM = 0.1;
  var MAX_ZOOM = 4;

  var DOMAINS = ['build', 'link', 'runtime'];

  /* ------------------------------------------------------------------ *
   * Mutable state and DOM handles
   * ------------------------------------------------------------------ */

  var model = null; // {nodes, nodesByName, edges, outEdges, inEdges}
  var layoutInfo = null; // {layers, width, height}
  var view = { x: 0, y: 0, k: 1 }; // pan (x/y in px) and zoom (k)
  var selectedName = null;
  var nodeEls = {}; // name -> <g class="node">
  var edgeEls = []; // [{g, from, to, domain}]
  var domainColors = {}; // marker colors, read back from CSS custom props

  var svg, viewport, edgesG, nodesG;
  var errorBanner, emptyEl, loadingEl, toastEl, detailEl, detailBody, searchEl, statsEl;

  /* ------------------------------------------------------------------ *
   * Small helpers
   * ------------------------------------------------------------------ */

  function el(tag, attrs) {
    var node = document.createElementNS(SVG_NS, tag);
    if (attrs) {
      for (var k in attrs) node.setAttribute(k, attrs[k]);
    }
    return node;
  }

  function esc(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function clamp(v, lo, hi) {
    return Math.min(hi, Math.max(lo, v));
  }

  function nameOf(n) {
    return n.name;
  }

  /* ------------------------------------------------------------------ *
   * Data loading
   * ------------------------------------------------------------------ */

  function loadGraph() {
    loadingEl.hidden = false;
    fetch('/api/graph')
      .then(function (res) {
        if (!res.ok) throw new Error('HTTP ' + res.status);
        return res.json();
      })
      .then(function (doc) {
        buildModel(doc);
        layout();
        render();
        clearSelection();
        applyFilters();
        applySearch();
        fitView();
        loadingEl.hidden = true;
        errorBanner.hidden = true;
        emptyEl.hidden = model.nodes.length !== 0;
        statsEl.textContent = model.nodes.length + ' nodes · ' + model.edges.length + ' edges';
      })
      .catch(function (err) {
        loadingEl.hidden = true;
        errorBanner.textContent = 'Failed to load /api/graph: ' + err.message;
        errorBanner.hidden = false;
      });
  }

  /* Index the document for quick lookups in both edge directions. */
  function buildModel(doc) {
    var nodes = doc.nodes || [];
    var edges = doc.edges || [];
    var nodesByName = {};
    var outEdges = {}; // from -> edges ("depends on")
    var inEdges = {}; // to   -> edges ("required by")
    nodes.forEach(function (n) {
      nodesByName[n.name] = n;
    });
    edges.forEach(function (e) {
      (outEdges[e.from] = outEdges[e.from] || []).push(e);
      (inEdges[e.to] = inEdges[e.to] || []).push(e);
    });
    model = { nodes: nodes, nodesByName: nodesByName, edges: edges, outEdges: outEdges, inEdges: inEdges };
  }

  /* ------------------------------------------------------------------ *
   * Layout: Kahn layering + barycenter ordering
   * ------------------------------------------------------------------ */

  function versionLabel(n) {
    if (n.state === 'external') return 'external';
    return n.version ? n.version + '-' + n.release : 'r' + n.release;
  }

  /* Box width is estimated from the longer of the two text lines. */
  function nodeWidth(n) {
    var w =
      Math.max(n.name.length * NAME_CHAR_W, versionLabel(n).length * VER_CHAR_W) + NODE_PAD_X * 2;
    return Math.max(NODE_MIN_W, Math.ceil(w));
  }

  function layout() {
    model.nodes.forEach(function (n) {
      n._w = nodeWidth(n);
      n._h = NODE_H;
    });
    var layers = assignLayers();
    orderLayers(layers);
    positionNodes(layers);
  }

  /* Layer of a node = length of the longest dependency chain to its left
   * (edge from->to means "from depends on to", so from lands in an earlier
   * layer and arrows point right). Nodes left over by cycles are appended
   * as one final layer so nothing ever loops forever. */
  function assignLayers() {
    var indeg = {}; // number of unplaced dependents pointing at a node
    model.nodes.forEach(function (n) {
      indeg[n.name] = 0;
    });
    model.edges.forEach(function (e) {
      if (indeg[e.to] !== undefined && e.from !== e.to) indeg[e.to]++;
    });

    var placed = {};
    var layers = [];
    var current = model.nodes
      .filter(function (n) {
        return indeg[n.name] === 0;
      })
      .map(nameOf)
      .sort();

    while (current.length) {
      layers.push(current);
      var next = [];
      current.forEach(function (name) {
        placed[name] = true;
        (model.outEdges[name] || []).forEach(function (e) {
          if (indeg[e.to] === undefined) return;
          indeg[e.to]--;
          if (indeg[e.to] === 0) next.push(e.to);
        });
      });
      next.sort();
      current = next;
    }

    var leftover = model.nodes
      .filter(function (n) {
        return !placed[n.name];
      })
      .map(nameOf)
      .sort();
    if (leftover.length) layers.push(leftover);
    return layers;
  }

  /* Reduce edge crossings: repeatedly sweep the layers, sorting each one by
   * the barycenter (average position) of its neighbors in the previous
   * layer of the sweep direction. Nodes without such neighbors stay put. */
  function orderLayers(layers) {
    if (layers.length < 2) return; // nothing to reorder (also guards empty graphs)
    var neighbors = {};
    model.edges.forEach(function (e) {
      (neighbors[e.from] = neighbors[e.from] || []).push(e.to);
      (neighbors[e.to] = neighbors[e.to] || []).push(e.from);
    });

    for (var pass = 0; pass < BARYCENTER_PASSES; pass++) {
      var forward = pass % 2 === 0;
      var start = forward ? 1 : layers.length - 2;
      var stop = forward ? layers.length : -1;
      var step = forward ? 1 : -1;
      for (var i = start; i !== stop; i += step) {
        var refIndex = {};
        layers[i - step].forEach(function (name, idx) {
          refIndex[name] = idx;
        });
        var order = layers[i].map(function (name, idx) {
          var positions = (neighbors[name] || [])
            .filter(function (nb) {
              return refIndex[nb] !== undefined;
            })
            .map(function (nb) {
              return refIndex[nb];
            });
          var b = idx;
          if (positions.length) {
            b =
              positions.reduce(function (a, c) {
                return a + c;
              }, 0) / positions.length;
          }
          return { name: name, b: b, idx: idx };
        });
        order.sort(function (a, b) {
          return a.b - b.b || a.idx - b.idx;
        });
        layers[i] = order.map(function (o) {
          return o.name;
        });
      }
    }
  }

  /* Assign x per layer (cumulative, widest node wins) and y per position
   * (layers vertically centered against the tallest one). */
  function positionNodes(layers) {
    var heights = layers.map(function (layer) {
      return layer.length * (NODE_H + ROW_GAP) - ROW_GAP;
    });
    var maxH = Math.max.apply(null, heights.concat([0]));
    var x = 0;
    layers.forEach(function (layer, li) {
      var w = 0;
      layer.forEach(function (name) {
        w = Math.max(w, model.nodesByName[name]._w);
      });
      var y0 = (maxH - heights[li]) / 2;
      layer.forEach(function (name, idx) {
        var n = model.nodesByName[name];
        n._x = x;
        n._y = y0 + idx * (NODE_H + ROW_GAP);
      });
      x += w + LAYER_GAP;
    });
    layoutInfo = { layers: layers, width: Math.max(x - LAYER_GAP, 1), height: Math.max(maxH, 1) };
  }

  /* ------------------------------------------------------------------ *
   * Rendering (plain SVG, rebuilt wholesale on each load)
   * ------------------------------------------------------------------ */

  /* Arrow markers cannot inherit the edge stroke color reliably across
   * browsers, so they are painted with the same CSS custom properties the
   * edge classes use. */
  function readThemeColors() {
    var cs = getComputedStyle(document.documentElement);
    DOMAINS.forEach(function (d) {
      domainColors[d] = cs.getPropertyValue('--edge-' + d).trim() || '#888';
    });
  }

  /* Cubic edge from the right edge of `a` to the left edge of `b`. When the
   * target sits at or behind the source (cycle leftovers in the trailing
   * layer), the curve bows upwards instead of folding back on itself. */
  function edgePath(a, b, spread) {
    var x1 = a._x + a._w;
    var y1 = a._y + a._h / 2 + spread;
    var x2 = b._x;
    var y2 = b._y + b._h / 2 + spread;
    if (x2 - x1 > 24) {
      var dx = Math.max(40, (x2 - x1) / 2);
      return 'M' + x1 + ' ' + y1 + ' C' + (x1 + dx) + ' ' + y1 + ' ' + (x2 - dx) + ' ' + y2 + ' ' + x2 + ' ' + y2;
    }
    var bow = 70;
    return 'M' + x1 + ' ' + y1 + ' C' + (x1 + bow) + ' ' + (y1 - bow) + ' ' + (x2 - bow) + ' ' + (y2 - bow) + ' ' + x2 + ' ' + y2;
  }

  function tooltipFor(n) {
    var lines = [n.name];
    lines.push(n.state === 'external' ? 'external reference' : versionLabel(n) + ' [' + n.state + ']');
    if (n.description) lines.push(n.description);
    return lines.join('\n');
  }

  function render() {
    readThemeColors();
    while (svg.firstChild) svg.removeChild(svg.firstChild);

    var defs = el('defs');
    DOMAINS.forEach(function (d) {
      var marker = el('marker', {
        id: 'arrow-' + d,
        viewBox: '0 0 10 10',
        refX: 9,
        refY: 5,
        markerWidth: 7,
        markerHeight: 7,
        orient: 'auto-start-reverse',
      });
      var arrow = el('path', { d: 'M 0 0 L 10 5 L 0 10 z' });
      arrow.style.fill = domainColors[d];
      marker.appendChild(arrow);
      defs.appendChild(marker);
    });
    svg.appendChild(defs);

    viewport = el('g', { id: 'viewport' });
    edgesG = el('g', { class: 'edges' });
    nodesG = el('g', { class: 'nodes' });
    viewport.appendChild(edgesG);
    viewport.appendChild(nodesG);
    svg.appendChild(viewport);

    nodeEls = {};
    edgeEls = [];

    // Count edges per (from,to) pair so parallel edges (same pair in
    // several domains) fan out instead of stacking invisibly.  cannot
    // appear in plan names, so the pair key is unambiguous.
    var pairCount = {};
    model.edges.forEach(function (e) {
      var key = e.from + '\u0001' + e.to;
      pairCount[key] = (pairCount[key] || 0) + 1;
    });
    var pairSeen = {};

    model.edges.forEach(function (e) {
      var a = model.nodesByName[e.from];
      var b = model.nodesByName[e.to];
      if (!a || !b) return;
      var key = e.from + '\u0001' + e.to;
      var idx = (pairSeen[key] = (pairSeen[key] || 0) + 1);
      var spread = (idx - (pairCount[key] + 1) / 2) * PARALLEL_SPREAD;
      var d = edgePath(a, b, spread);
      var g = el('g', { class: 'edge dm-' + e.domain });
      g.appendChild(el('path', { d: d, class: 'edge-hit' }));
      g.appendChild(
        el('path', { d: d, class: 'edge-line', 'marker-end': 'url(#arrow-' + e.domain + ')' })
      );
      var title = el('title');
      title.textContent = e.from + ' → ' + e.to + ' (' + e.domain + ')';
      g.appendChild(title);
      edgesG.appendChild(g);
      edgeEls.push({ g: g, from: e.from, to: e.to, domain: e.domain });
    });

    model.nodes.forEach(function (n) {
      var g = el('g', {
        class: 'node st-' + n.state,
        transform: 'translate(' + n._x + ' ' + n._y + ')',
        'data-name': n.name,
        tabindex: '0',
        role: 'button',
        'aria-label': n.name + ' (' + n.state + ')',
      });
      g.appendChild(el('rect', { width: n._w, height: n._h, rx: 7 }));
      var name = el('text', { class: 'node-name', x: n._w / 2, y: 17 });
      name.textContent = n.name;
      var ver = el('text', { class: 'node-ver', x: n._w / 2, y: 31 });
      ver.textContent = versionLabel(n);
      var title = el('title');
      title.textContent = tooltipFor(n);
      g.appendChild(name);
      g.appendChild(ver);
      g.appendChild(title);
      nodesG.appendChild(g);
      nodeEls[n.name] = g;
    });

    applyView();
  }

  /* ------------------------------------------------------------------ *
   * Pan / zoom viewport
   * ------------------------------------------------------------------ */

  function applyView() {
    viewport.setAttribute(
      'transform',
      'translate(' + view.x + ' ' + view.y + ') scale(' + view.k + ')'
    );
  }

  /* Fit the whole graph into the visible canvas (never enlarging past 1:1). */
  function fitView() {
    if (!layoutInfo || !model.nodes.length) {
      view = { x: 0, y: 0, k: 1 };
      applyView();
      return;
    }
    var w = svg.clientWidth || 1;
    var h = svg.clientHeight || 1;
    var k = clamp(Math.min(w / (layoutInfo.width + 80), h / (layoutInfo.height + 80)), MIN_ZOOM, 1);
    view.k = k;
    view.x = (w - layoutInfo.width * k) / 2;
    view.y = (h - layoutInfo.height * k) / 2;
    applyView();
  }

  function centerOn(name) {
    var n = model.nodesByName[name];
    if (!n) return;
    var rect = svg.getBoundingClientRect();
    view.x = rect.width / 2 - (n._x + n._w / 2) * view.k;
    view.y = rect.height / 2 - (n._y + n._h / 2) * view.k;
    applyView();
  }

  var drag = null;
  var suppressClick = false; // a real pan must not count as a background click

  function bindViewportEvents() {
    svg.addEventListener(
      'wheel',
      function (evt) {
        if (!model) return;
        evt.preventDefault();
        var rect = svg.getBoundingClientRect();
        var mx = evt.clientX - rect.left;
        var my = evt.clientY - rect.top;
        var k2 = clamp(view.k * Math.exp(-evt.deltaY * 0.0015), MIN_ZOOM, MAX_ZOOM);
        // Keep the graph point under the cursor fixed while zooming.
        view.x = mx - (mx - view.x) * (k2 / view.k);
        view.y = my - (my - view.y) * (k2 / view.k);
        view.k = k2;
        applyView();
      },
      { passive: false }
    );

    svg.addEventListener('pointerdown', function (evt) {
      if (evt.button !== 0) return;
      drag = { x: evt.clientX, y: evt.clientY, moved: false };
      svg.setPointerCapture(evt.pointerId);
    });

    svg.addEventListener('pointermove', function (evt) {
      if (!drag) return;
      var dx = evt.clientX - drag.x;
      var dy = evt.clientY - drag.y;
      if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
      if (drag.moved) {
        view.x += dx;
        view.y += dy;
        drag.x = evt.clientX;
        drag.y = evt.clientY;
        svg.classList.add('panning');
        applyView();
      }
    });

    svg.addEventListener('pointerup', function (evt) {
      if (!drag) return;
      svg.releasePointerCapture(evt.pointerId);
      if (drag.moved) {
        suppressClick = true;
        setTimeout(function () {
          suppressClick = false;
        }, 0);
      }
      drag = null;
      svg.classList.remove('panning');
    });

    svg.addEventListener('dblclick', function () {
      fitView();
    });

    // One delegated click handler: node -> select, anywhere else -> close.
    svg.addEventListener('click', function (evt) {
      if (suppressClick || !model) return;
      var g = evt.target.closest ? evt.target.closest('.node') : null;
      if (g && nodesG.contains(g)) selectNode(g.getAttribute('data-name'), false);
      else clearSelection();
    });

    svg.addEventListener('keydown', function (evt) {
      if (evt.key !== 'Enter' && evt.key !== ' ') return;
      var g = evt.target.closest ? evt.target.closest('.node') : null;
      if (g && nodesG.contains(g)) {
        evt.preventDefault();
        selectNode(g.getAttribute('data-name'), false);
      }
    });
  }

  /* ------------------------------------------------------------------ *
   * Filters and search (hide/dim in place; the layout never reflows)
   * ------------------------------------------------------------------ */

  function checkedSet(groupId) {
    var set = {};
    var boxes = document.getElementById(groupId).querySelectorAll('input[type="checkbox"]');
    boxes.forEach(function (box) {
      set[box.value] = box.checked;
    });
    return set;
  }

  /* Hide nodes by state and edges by domain; an edge also disappears when
   * either endpoint is hidden. */
  function applyFilters() {
    if (!model) return;
    var states = checkedSet('state-filters');
    var domains = checkedSet('domain-filters');
    model.nodes.forEach(function (n) {
      nodeEls[n.name].classList.toggle('hidden', !states[n.state]);
    });
    edgeEls.forEach(function (rec) {
      var hide =
        !domains[rec.domain] ||
        !states[model.nodesByName[rec.from].state] ||
        !states[model.nodesByName[rec.to].state];
      rec.g.classList.toggle('hidden', hide);
    });
  }

  /* Matching nodes plus their direct dependencies/dependents stay at full
   * opacity; everything else is dimmed. Empty query restores the view. */
  function applySearch() {
    if (!model) return;
    var q = searchEl.value.trim().toLowerCase();
    if (!q) {
      model.nodes.forEach(function (n) {
        nodeEls[n.name].classList.remove('dim');
      });
      edgeEls.forEach(function (rec) {
        rec.g.classList.remove('dim');
      });
      return;
    }
    var keep = {};
    model.nodes.forEach(function (n) {
      if (n.name.toLowerCase().indexOf(q) === -1) return;
      keep[n.name] = true;
      (model.outEdges[n.name] || []).forEach(function (e) {
        keep[e.to] = true;
      });
      (model.inEdges[n.name] || []).forEach(function (e) {
        keep[e.from] = true;
      });
    });
    model.nodes.forEach(function (n) {
      nodeEls[n.name].classList.toggle('dim', !keep[n.name]);
    });
    edgeEls.forEach(function (rec) {
      rec.g.classList.toggle('dim', !(keep[rec.from] && keep[rec.to]));
    });
  }

  /* ------------------------------------------------------------------ *
   * Selection and the detail panel
   * ------------------------------------------------------------------ */

  function selectNode(name, recenter) {
    if (!model || !model.nodesByName[name]) return;
    if (recenter) {
      if (nodeEls[name].classList.contains('hidden')) {
        toast('"' + name + '" is hidden by the current filters');
        return;
      }
      centerOn(name);
    }
    if (selectedName && nodeEls[selectedName]) {
      nodeEls[selectedName].classList.remove('selected');
    }
    selectedName = name;
    nodeEls[name].classList.add('selected');
    applyEdgeHighlight();
    renderDetail(name);
    detailEl.hidden = false;
  }

  function clearSelection() {
    if (selectedName && nodeEls[selectedName]) {
      nodeEls[selectedName].classList.remove('selected');
    }
    selectedName = null;
    applyEdgeHighlight();
    detailEl.hidden = true;
  }

  /* Bold every edge touching the selected node (edge hover is pure CSS). */
  function applyEdgeHighlight() {
    edgeEls.forEach(function (rec) {
      rec.g.classList.toggle(
        'edge-hl',
        !!selectedName && (rec.from === selectedName || rec.to === selectedName)
      );
    });
  }

  /* A <div>, not a <p>: the value wraps in its own block, and <div> inside
   * <p> would be ejected by the HTML parser and lose the .kv styling. */
  function kv(label, valueHtml) {
    return '<div class="kv"><span>' + label + '</span><div>' + valueHtml + '</div></div>';
  }

  function chipList(title, items, linkify) {
    var html = title ? '<h3>' + title + '</h3>' : '';
    html += '<ul class="chiplist">';
    items.forEach(function (item) {
      html +=
        '<li>' +
        (linkify ? '<a href="#" data-node="' + esc(item) + '">' + esc(item) + '</a>' : esc(item)) +
        '</li>';
    });
    return html + '</ul>';
  }

  function renderDetail(name) {
    var n = model.nodesByName[name];
    var html = '<h2>' + esc(n.name) + '</h2>';
    html += '<p class="badge st-' + n.state + '">' + n.state + '</p>';

    if (n.state === 'external') {
      html += '<p class="muted">external reference — provided outside the plan set</p>';
    } else {
      html += kv('version', n.version ? esc(n.version) : '—');
      html += kv('release', String(n.release));
    }
    if (n.description) html += kv('description', esc(n.description));
    if (n.url && /^https?:\/\//i.test(n.url)) {
      html += kv(
        'url',
        '<a href="' + esc(n.url) + '" target="_blank" rel="noopener noreferrer">' + esc(n.url) + '</a>'
      );
    }
    if (n.outputs && n.outputs.length) html += chipList('outputs', n.outputs, false);

    var out = model.outEdges[name] || [];
    if (out.length) {
      html += '<h3>depends on</h3>';
      DOMAINS.forEach(function (domain) {
        var names = out
          .filter(function (e) {
            return e.domain === domain;
          })
          .map(function (e) {
            return e.to;
          });
        if (names.length) {
          html += '<p class="dep-domain dm-' + domain + '-text">' + domain + '</p>';
          html += chipList(null, names, true);
        }
      });
    }

    var incoming = model.inEdges[name] || [];
    if (incoming.length) {
      html += '<h3>required by</h3><ul class="chiplist">';
      incoming.forEach(function (e) {
        html +=
          '<li><a href="#" data-node="' +
          esc(e.from) +
          '">' +
          esc(e.from) +
          '</a><span class="dm-tag dm-' +
          e.domain +
          '-text">' +
          e.domain +
          '</span></li>';
      });
      html += '</ul>';
    }

    if (n.replaces && n.replaces.length) html += chipList('replaces', n.replaces, false);
    if (n.conflicts && n.conflicts.length) html += chipList('conflicts', n.conflicts, false);

    detailBody.innerHTML = html;
  }

  var toastTimer = null;

  function toast(msg) {
    toastEl.textContent = msg;
    toastEl.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      toastEl.hidden = true;
    }, 3000);
  }

  /* ------------------------------------------------------------------ *
   * Wiring
   * ------------------------------------------------------------------ */

  function init() {
    svg = document.getElementById('graph');
    errorBanner = document.getElementById('error-banner');
    emptyEl = document.getElementById('empty');
    loadingEl = document.getElementById('loading');
    toastEl = document.getElementById('toast');
    detailEl = document.getElementById('detail');
    detailBody = document.getElementById('detail-body');
    searchEl = document.getElementById('search');
    statsEl = document.getElementById('stats');

    document.getElementById('refresh').addEventListener('click', loadGraph);
    document.getElementById('detail-close').addEventListener('click', clearSelection);
    searchEl.addEventListener('input', applySearch);
    document.getElementById('domain-filters').addEventListener('change', applyFilters);
    document.getElementById('state-filters').addEventListener('change', applyFilters);

    // Dependency links inside the detail panel jump to the target node.
    detailBody.addEventListener('click', function (evt) {
      var a = evt.target.closest ? evt.target.closest('a[data-node]') : null;
      if (!a) return;
      evt.preventDefault();
      selectNode(a.getAttribute('data-node'), true);
    });

    document.addEventListener('keydown', function (evt) {
      if (evt.key === 'Escape') clearSelection();
    });

    bindViewportEvents();
    loadGraph();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
