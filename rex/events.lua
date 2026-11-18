-- paddock's event listener on a Rex server, run by src/backend/rex.rs as
-- `rex do -e … --all`: one line for each burst of events that can change
-- paddock's snapshot, which paddock answers with a new one. A burst is what
-- comes in WAIT. The line is "blocks ID…" when only those blocks' state,
-- title or folder changed, and only they need reading again; "changed ID…"
-- when sessions, tabs or layouts did too (the panes paddock has are as it
-- read them, but for the ones named); "all" when a pane no event names
-- changed, and every one needs reading.
--
-- Two more lines are no news: "ready" once it listens (what happened before
-- is in no event, so paddock reads everything then), and "alive" every
-- ALIVE seconds, written to find out that paddock is gone: a listener
-- outlives a paddock that was killed until it next writes, which on a quiet
-- server is never.
--
-- Agents animate their titles several times a second, a spinner or a glyph
-- in front of the same words: a title counts only when what paddock shows
-- of it changes.

local EVENTS = {
  "session_created",
  "session_destroyed",
  "session_label_changed",
  "session_moved",
  "window_created",
  "window_closed",
  "window_label_changed",
  "active_window_changed",
  "session_view_changed",
}

local BLOCK_EVENTS = {
  "terminal.program_status_changed",
  "terminal.program_status_removed",
  "terminal.pwd_changed",
  "terminal.process_changed",
}

local WAIT = 0.15
local ALIVE = 10

-- LINE to paddock; with no one to read it, the listener ends.
local function say(line)
  if not io.write(line, "\n") or io.flush() == nil then rex.stop(0) end
end
-- The burst so far: whether sessions, tabs or layouts changed, the blocks
-- that did, and whether one no event names did.
local pending, layout, blocks, any = false, false, {}, false

-- The layout changed, or (in a block's event) BLOCK did; without its block,
-- the event could be about any.
local function changed(block, event)
  if block then
    blocks[block] = true
  elseif event then
    any = true
  else
    layout = true
  end
  if pending then return end
  pending = true
  rex.after(WAIT, function()
    local line = "all"
    if not any then
      local words = { layout and "changed" or "blocks" }
      for id in pairs(blocks) do words[#words + 1] = id end
      line = table.concat(words, " ")
    end
    pending, layout, blocks, any = false, false, {}, false
    say(line)
  end)
end

for _, name in ipairs(EVENTS) do
  rex.on(name, function() changed() end)
end
for _, name in ipairs(BLOCK_EVENTS) do
  rex.on(name, function(_, event)
    changed(type(event) == "table" and event.block_id or nil, true)
  end)
end

-- TITLE as paddock shows it (model::title_of): without the spaces and
-- symbols in front (U+2000 to U+2BFF: spinners, ✳, ●; U+1F000 on: emoji),
-- nor spaces at the end. ASCII punctuation stays (~/src, [wip]), and so do
-- the letters and digits among those symbols (superscripts, ℂ, Ⅻ, ①, Ⓐ, ❶,
-- 🄰), as title_of keeps them: it may keep more than title_of, never less,
-- so no change shown is missed.
local function letters(cp)
  return (cp >= 0x2070 and cp <= 0x218F) or (cp >= 0x2460 and cp <= 0x24FF) or (cp >= 0x2776 and cp <= 0x2793)
    or (cp >= 0x1F100 and cp <= 0x1F1FF) or (cp >= 0x1FB00 and cp <= 0x1FBFF)
end

local function bare(title)
  local i = 1
  while true do
    local b = title:byte(i)
    if not b then break end
    local b2, b3, b4 = title:byte(i + 1, i + 3)
    if b < 0x80 then
      if not title:sub(i, i):match("%s") then break end
      i = i + 1
    elseif b == 0xE2 and b2 and b3 and b2 <= 0xAF and not letters(0x2000 + (b2 - 0x80) * 64 + (b3 - 0x80)) then
      i = i + 3
    elseif b == 0xF0 and b2 == 0x9F and b3 and b4 and not letters(0x1F000 + (b3 - 0x80) * 64 + (b4 - 0x80)) then
      i = i + 4
    else
      break
    end
  end
  return (title:sub(i):gsub("%s+$", ""))
end

-- Each block's title, as last shown, until the block closes.
local titles = {}
rex.on("terminal.title_changed", function(_, event)
  local title = bare(event.title or "")
  if titles[event.block_id] ~= title then
    titles[event.block_id] = title
    changed(event.block_id, true)
  end
end)
rex.on("block_closed", function(_, event)
  titles[event.block_id] = nil
end)

local function alive()
  say("alive")
  rex.after(ALIVE, alive)
end
rex.after(ALIVE, alive)
-- From the event loop: the handlers above are heard by then (no wait: an
-- event caused the moment paddock reads "ready" is heard).
rex.after(0, function() say("ready") end)
