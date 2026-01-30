let FOLDERS = [];
let BOARD = [];
const board = document.getElementById("board");
const template = document.getElementById("task-card-template");
const form = document.getElementById("task-form");
const submitButton = form.querySelector("button[type='submit']");
let editingTaskId = null;
let lastSnapshot = "";
let lastBoardSnapshot = "";
const AUTO_REFRESH_MS = 5000;
const boardForm = document.getElementById("board-form");
const boardRows = document.getElementById("board-rows");
const boardRowTemplate = document.getElementById("board-row-template");
const addColumnButton = document.getElementById("add-column");
const toggleEditorButton = document.getElementById("toggle-editor");
const boardEditor = document.getElementById("board-editor");
const taskEditor = document.getElementById("task-editor");
const toggleTaskEditorButton = document.getElementById("toggle-task-editor");
const headline = document.getElementById("headline");

async function api(path, options = {}) {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || "Request failed");
  }
  if (res.status === 204) return null;
  return res.json();
}

function formatMeta(label, value) {
  if (!value) return "";
  return `${label}: ${value}`;
}

function renderTask(task) {
  const card = template.content.firstElementChild.cloneNode(true);
  card.dataset.id = task.id;
  card.dataset.folder = task.folder;
  card.querySelector(".card-title").textContent = task.title || task.id;
  card.querySelector(".card-description").textContent = task.description || "";
  const creator = card.querySelector("[data-meta='creator']");
  creator.textContent = formatMeta("creator", task.creator);
  const assigned = card.querySelector("[data-meta='assigned_to']");
  assigned.textContent = formatMeta("assigned", task.assigned_to);
  card.querySelector("[data-meta='updated_at']").textContent = task.updated_at ? `updated: ${task.updated_at}` : "";
  card.querySelector("[data-meta='id']").textContent = task.id;

  const tagsWrap = card.querySelector(".card-tags");
  tagsWrap.innerHTML = "";
  (task.tags || []).forEach((tag) => {
    const span = document.createElement("span");
    span.textContent = tag;
    tagsWrap.appendChild(span);
  });

  card.addEventListener("dragstart", (event) => {
    event.dataTransfer.setData("text/plain", task.id);
  });

  card.querySelector("[data-action='edit']").addEventListener("click", () => {
    form.title.value = task.title || "";
    form.creator.value = task.creator || "";
    form.assigned_to.value = task.assigned_to || "";
    form.tags.value = (task.tags || []).join(", ");
    form.description.value = task.description || "";
    editingTaskId = task.id;
    submitButton.textContent = "Update task";
    form.scrollIntoView({ behavior: "smooth", block: "start" });
  });

  card.querySelector("[data-action='delete']").addEventListener("click", async () => {
    if (!confirm(`Delete ${task.title}?`)) return;
    await api(`/api/tasks/${task.id}`, { method: "DELETE" });
    await loadTasks();
  });

  return card;
}

function renderBoard(columns) {
  board.innerHTML = "";
  columns.forEach((column) => {
    const section = document.createElement("section");
    section.className = "column";
    section.dataset.folder = column.id;
    if (column.wip_limit) {
      section.dataset.wip = String(column.wip_limit);
    }
    section.innerHTML = `
      <header>
        <h3>${column.title}</h3>
        <span class="count" data-count="${column.id}">0</span>
      </header>
      <div class="column-body" data-dropzone="${column.id}"></div>
    `;
    board.appendChild(section);
  });
}

function renderBoardEditor(columns) {
  boardRows.innerHTML = "";
  columns.forEach((column) => {
    const row = boardRowTemplate.content.firstElementChild.cloneNode(true);
    row.querySelector("input[name='id']").value = column.id;
    row.querySelector("input[name='title']").value = column.title;
    row.querySelector("input[name='wip_limit']").value = column.wip_limit || 0;
    row.querySelector("[data-action='remove']").addEventListener("click", () => {
      row.remove();
    });
    boardRows.appendChild(row);
  });
}

function setEditorVisibility(target, isVisible, button, labels) {
  if (isVisible) {
    target.classList.remove("hidden");
    button.textContent = labels.hide;
  } else {
    target.classList.add("hidden");
    button.textContent = labels.show;
  }
}

function readUiPreference(key) {
  const value = localStorage.getItem(key);
  if (value === null) return null;
  return value === "true";
}

function writeUiPreference(key, value) {
  localStorage.setItem(key, value ? "true" : "false");
}

async function loadUiDefaults() {
  try {
    const taskPref = readUiPreference("kanban.showTaskEditor");
    const boardPref = readUiPreference("kanban.showBoardEditor");
    if (taskPref !== null && boardPref !== null) {
      setEditorVisibility(taskEditor, taskPref, toggleTaskEditorButton, {
        show: "Show task editor",
        hide: "Hide task editor",
      });
      setEditorVisibility(boardEditor, boardPref, toggleEditorButton, {
        show: "Show editor",
        hide: "Hide editor",
      });
      return;
    }
    const data = await api("/api/ui");
    setEditorVisibility(taskEditor, data.show_task_editor, toggleTaskEditorButton, {
      show: "Show task editor",
      hide: "Hide task editor",
    });
    setEditorVisibility(boardEditor, data.show_board_editor, toggleEditorButton, {
      show: "Show editor",
      hide: "Hide editor",
    });
    writeUiPreference("kanban.showTaskEditor", data.show_task_editor);
    writeUiPreference("kanban.showBoardEditor", data.show_board_editor);
  } catch (err) {
    console.warn("Failed to load UI defaults", err);
  }
}

async function loadThemeSettings() {
  try {
    const data = await api("/api/theme");
    const theme = data.theme || {};
    if (theme.headline) {
      headline.textContent = theme.headline;
      document.title = theme.headline;
    }
    const colors = theme.colors || {};
    Object.entries(colors).forEach(([key, value]) => {
      document.documentElement.style.setProperty(`--${key.replace(/_/g, "-")}`, value);
    });
  } catch (err) {
    console.warn("Failed to load theme settings", err);
  }
}

function getCardRects() {
  const rects = new Map();
  document.querySelectorAll(".card").forEach((card) => {
    rects.set(card.dataset.id, card.getBoundingClientRect());
  });
  return rects;
}

function animateCards(previousRects) {
  document.querySelectorAll(".card").forEach((card) => {
    const prev = previousRects.get(card.dataset.id);
    if (!prev) {
      card.animate([{ opacity: 0, transform: "scale(0.98)" }, { opacity: 1, transform: "scale(1)" }], {
        duration: 180,
        easing: "cubic-bezier(0.25, 0.1, 0.25, 1)",
      });
      return;
    }
    const next = card.getBoundingClientRect();
    const dx = prev.left - next.left;
    const dy = prev.top - next.top;
    if (dx !== 0 || dy !== 0) {
      card.animate(
        [
          { transform: `translate(${dx}px, ${dy}px)` },
          { transform: "translate(0, 0)" },
        ],
        { duration: 240, easing: "cubic-bezier(0.25, 0.1, 0.25, 1)" }
      );
    }
  });
}

async function loadTasks() {
  const previousRects = getCardRects();
  const data = await api("/api/tasks");
  const snapshot = JSON.stringify(data.folders || {});
  const boardSnapshot = JSON.stringify(data.board || {});
  const boardChanged = boardSnapshot !== lastBoardSnapshot;
  if (boardChanged) {
    lastBoardSnapshot = boardSnapshot;
    BOARD = (data.board && data.board.columns) || [];
    FOLDERS = BOARD.map((c) => c.id);
    renderBoard(BOARD);
    renderBoardEditor(BOARD);
    setupDropzones();
  }
  if (!boardChanged && snapshot === lastSnapshot) return;
  lastSnapshot = snapshot;
  FOLDERS.forEach((folder) => {
    const column = board.querySelector(`[data-dropzone='${folder}']`);
    column.innerHTML = "";
    const tasks = (data.folders && data.folders[folder]) || [];
    tasks.forEach((task) => column.appendChild(renderTask(task)));
    const count = document.querySelector(`[data-count='${folder}']`);
    const section = board.querySelector(`[data-folder='${folder}']`);
    if (section) {
      const limit = Number(section.dataset.wip || 0);
      if (limit > 0 && tasks.length > limit) {
        section.classList.add("wip-over");
      } else {
        section.classList.remove("wip-over");
      }
      if (count) {
        count.textContent = limit > 0 ? `${tasks.length}/${limit}` : `${tasks.length}`;
      }
    } else if (count) {
      count.textContent = `${tasks.length}`;
    }
  });
  animateCards(previousRects);
}

function setupDropzones() {
  document.querySelectorAll(".column-body").forEach((zone) => {
    zone.addEventListener("dragover", (event) => {
      event.preventDefault();
      zone.classList.add("dragover");
    });
    zone.addEventListener("dragleave", () => zone.classList.remove("dragover"));
    zone.addEventListener("drop", async (event) => {
      event.preventDefault();
      zone.classList.remove("dragover");
      const id = event.dataTransfer.getData("text/plain");
      if (!id) return;
      const folder = zone.dataset.dropzone;
      await api(`/api/tasks/${id}/move`, {
        method: "POST",
        body: JSON.stringify({ folder }),
      });
      await loadTasks();
    });
  });
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (FOLDERS.length === 0) {
    await loadTasks();
    if (FOLDERS.length === 0) {
      alert("Board configuration is missing.");
      return;
    }
  }
  const formData = new FormData(form);
  const payload = {
    title: formData.get("title"),
    description: formData.get("description") || "",
    creator: formData.get("creator") || "",
    assigned_to: formData.get("assigned_to") || "",
    tags: (formData.get("tags") || "")
      .split(",")
      .map((t) => t.trim())
      .filter(Boolean),
    status: FOLDERS[0] || "backlog",
  };
  if (editingTaskId) {
    await api(`/api/tasks/${editingTaskId}`, { method: "PUT", body: JSON.stringify(payload) });
    editingTaskId = null;
    submitButton.textContent = "Add to backlog";
  } else {
    await api("/api/tasks", { method: "POST", body: JSON.stringify(payload) });
  }
  form.reset();
  await loadTasks();
});

loadTasks().catch((err) => {
  console.error(err);
  alert("Failed to load tasks. Is the backend running?");
});

loadUiDefaults();
loadThemeSettings();

setInterval(() => {
  loadTasks().catch((err) => console.warn("Auto-refresh failed", err));
}, AUTO_REFRESH_MS);

addColumnButton.addEventListener("click", () => {
  const row = boardRowTemplate.content.firstElementChild.cloneNode(true);
  row.querySelector("[data-action='remove']").addEventListener("click", () => {
    row.remove();
  });
  boardRows.appendChild(row);
});

boardForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const rows = Array.from(boardRows.querySelectorAll(".board-row"));
  const columns = rows
    .map((row) => {
      const id = row.querySelector("input[name='id']").value.trim();
      const title = row.querySelector("input[name='title']").value.trim();
      const wipRaw = row.querySelector("input[name='wip_limit']").value.trim();
      const wipLimit = Number.parseInt(wipRaw, 10);
      return {
        id,
        title: title || id,
        wip_limit: Number.isFinite(wipLimit) && wipLimit > 0 ? wipLimit : 0,
      };
    })
    .filter((col) => col.id.length > 0);

  if (columns.length === 0) {
    alert("Add at least one column.");
    return;
  }

  await api("/api/board", {
    method: "PUT",
    body: JSON.stringify({ columns }),
  });
  await loadTasks();
});

toggleEditorButton.addEventListener("click", () => {
  const isHidden = boardEditor.classList.toggle("hidden");
  const isVisible = !isHidden;
  toggleEditorButton.textContent = isHidden ? "Show editor" : "Hide editor";
  writeUiPreference("kanban.showBoardEditor", isVisible);
});

toggleTaskEditorButton.addEventListener("click", () => {
  const isHidden = taskEditor.classList.toggle("hidden");
  const isVisible = !isHidden;
  toggleTaskEditorButton.textContent = isHidden ? "Show task editor" : "Hide task editor";
  writeUiPreference("kanban.showTaskEditor", isVisible);
});
