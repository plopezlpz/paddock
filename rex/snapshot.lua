-- paddock's snapshot of a Rex server: every session, tab and terminal block,
-- as Rex reports them. src/backend/rex.rs turns it into paddock's model.
--
-- rex/worker.lua runs it for paddock. On its own it prints the snapshot as
-- JSON, for debugging:
--
--   rex do rex/snapshot.lua --json [skip=BLOCK_ID]
--
-- skip leaves out a block: paddock's own.

TERMINAL = "com.superlogical.terminal"

-- A connection reads and changes another session, and calls its blocks,
-- only once attached to it; block calls also name the session. An attach
-- that failed is tried again next time.
-- An attach answers with the session's view: the snapshot that attached
-- takes it, one call fewer per session (a round trip each on a far host).
local attached, views = {}, {}
function on(session)
  if session and not attached[session] then
    local a = rex.session.attach { session_id = session }
    attached[session] = a ~= nil
    views[session] = a and a.view
  end
  return session
end

-- Where the last snapshot found each block and tab: their session (and a
-- block's window), so requests about them need no search.
places = { blocks = {}, windows = {} }

function block_call(session, block, method, args)
  args = args or {}
  args.session_id = on(session)
  args.block_id = block
  return rex.block.call(TERMINAL, method, args)
end

-- The shells, as model::is_shell lists them in paddock.
local SHELLS = { sh = true, bash = true, zsh = true, fish = true, dash = true, ksh = true, mksh = true, tcsh = true, csh = true, nu = true }

-- The record that says what the program is doing: the most recent blocked
-- one (a sub-task, such as a subagent, may wait on the user while the root
-- works on), else the root record, else the most recent one; and whether the
-- call was answered at all (no record is an answer too). Records outlive
-- their program, so those of the process in front (PID) come first: a dead
-- agent's open question doesn't hide the next one's state. Without any,
-- the choice is made from them all, and paddock shows it unconfirmed.
local function status(session, block, pid)
  local answer = block_call(session, block, "program_status")
  local records = (answer or {}).records or {}
  local function current(r)
    local o = r.owner
    return not o or not o.pid or o.pid == 0 or o.pid == pid or SHELLS[(o.name or ""):gsub("^%-", "")] ~= nil
  end
  local function pick(only)
    local root, newest, blocked
    local function newer(r, than) return not than or (r.updated_at or "") > (than.updated_at or "") end
    for _, r in ipairs(records) do
      if not only or current(r) then
        if r.id == nil or r.id == "" then root = r end
        if newer(r, newest) then newest = r end
        if r.state == "blocked" and newer(r, blocked) then blocked = r end
      end
    end
    return blocked or root or newest
  end
  return (pid and pick(true)) or pick(false), answer ~= nil
end

-- Blocks and sessions last read with a call unanswered (a connection's
-- hiccup), and how many times in a row: what was read of them is not kept,
-- and paddock asks for them again (`unsure`), up to TRIES times.
local shaky, TRIES = {}, 3

-- Whether ID's read failed, counting the failures in a row; and whether
-- paddock should ask for it again.
local function failed(id, failing)
  shaky[id] = failing and (shaky[id] or 0) + 1 or nil
  return failing and shaky[id] <= TRIES or nil
end

-- BLOCK read, as a pane; what a call left unanswered is as HAD has it (the
-- pane as last read), not missing: a failed read is not an agent gone.
local function pane(session, block, active, had)
  had = had or {}
  local id = block.block_id
  local process = block_call(session, id, "process")
  -- The process call answered without a program in front (`alone`): none
  -- runs there to confirm a status.
  local fg = process and (process.foreground or { alone = true })
    or { pid = had.pid, argv0 = had.argv0, alone = had.alone }
  local cwd = fg.cwd or (block_call(session, id, "pwd") or {}).pwd or had.cwd
  local st, answered = status(session, id, fg.pid)
  if not answered then st = had.status end
  -- Only an agent's title is shown; one unanswered is as last read.
  local title = st and block_call(session, id, "title")
  return {
    id = id,
    active = active,
    label = block.label,
    title = title and title.title or (st and title == nil and had.title) or nil,
    argv0 = fg.argv0 or fg.name,
    pid = fg.pid,
    cwd = cwd,
    alone = fg.alone,
    status = st,
    unsure = failed(id, not process or not answered),
  }
end

-- The last snapshot, and where in it each pane came from, so that `refresh`
-- can read again only the blocks that changed; and the blocks read for the
-- request being answered, which need no second reading in it.
local last, listed
fresh = {}

-- Session ID's tabs, from its view V, and their panes: with KEEP, those
-- the last snapshot listed (WAS) are kept as it read them, but for shaky
-- ones; the others are read, and what a call leaves unanswered is as WAS
-- has it.
local function windows(id, v, skip, keep, was)
  local list = {}
  for _, w in ipairs(v.windows or {}) do
    places.windows[w.window_id] = id
    -- While paddock's own block has the focus, the block it opened from
    -- stands for it.
    local focused = w.focused_block_id
    if skip and focused == skip and (PADDOCK_FROM or "") ~= "" then focused = PADDOCK_FROM end
    local panes = {}
    for _, layer in ipairs(w.layers or {}) do
      for _, b in ipairs(layer.blocks or {}) do
        places.blocks[b.block_id] = { session = id, window = w.window_id }
        if b.creator_name == TERMINAL and b.block_id ~= skip then
          local had = was[b.block_id]
          had = had and had.panes[had.at]
          local p = keep and not shaky[b.block_id] and had
          if p then
            p.active, p.label = b.block_id == focused, b.label
          else
            p = pane(id, b, b.block_id == focused, had)
            fresh[b.block_id] = true
          end
          panes[#panes + 1] = p
          listed[b.block_id] = { session = id, block = b, panes = panes, at = #panes }
        end
      end
    end
    list[#list + 1] = { id = w.window_id, name = w.label, active = w.window_id == v.active_window_id, panes = panes }
  end
  return list
end

-- Every session, tab and terminal block. A pane costs two to four calls
-- (its process, folder, status and title), a session one: with KEEP, the
-- panes the last snapshot has are kept as it read them (the listener names
-- the ones that change), with the label and focus this one finds, and only
-- new ones are read. Over a network that is the difference between a
-- snapshot in a tenth of a second and one that takes that for each pane.
function snapshot(skip, keep)
  local was, was_places, was_sessions = listed or {}, places, {}
  for _, s in ipairs(last or {}) do was_sessions[s.id] = s end
  -- Failing, it says so: no sessions is not the same.
  local list, err = rex.call("session.list")
  if not list then error(tostring(err or "rex.call failed"), 0) end
  -- Only the views of attaches this snapshot makes are fresh.
  views = {}
  places = { blocks = {}, windows = {} }
  listed = {}
  local sessions, seen = {}, {}
  for _, s in ipairs(list.sessions or {}) do
    local id = on(s.session_id)
    seen[id] = true
    local v = views[id] or rex.session.view { session_id = id }
    views[id] = nil
    if v then
      failed(id, false)
      sessions[#sessions + 1] = { id = id, name = s.label, windows = windows(id, v, skip, keep, was) }
    else
      -- Unread, a session keeps its tabs as last read (none, if it is
      -- new): a failed read is not a session emptied.
      local had = was_sessions[id] or { windows = {} }
      for _, w in ipairs(had.windows) do
        places.windows[w.id] = id
        for _, p in ipairs(w.panes) do
          places.blocks[p.id], listed[p.id] = was_places.blocks[p.id], was[p.id]
          -- A whole snapshot reads every pane: the retry does these.
          if not keep then shaky[p.id] = shaky[p.id] or 0 end
        end
      end
      sessions[#sessions + 1] = { id = id, name = s.label, windows = had.windows, unsure = failed(id, true) }
    end
  end
  for id in pairs(shaky) do
    if not listed[id] and not seen[id] then shaky[id] = nil end
  end
  last = sessions
  return sessions
end

-- The last snapshot with BLOCKS read again: their state, title and folder.
-- A block it does not have is new: the sessions and tabs are read again,
-- and the panes they have that it has not.
function refresh(skip, blocks)
  if not last then return snapshot(skip) end
  local looked = false
  for _, id in ipairs(blocks) do
    if not listed[id] and not looked and id ~= skip and not places.blocks[id] then
      looked = true
      snapshot(skip, true)
    end
    local p = listed[id]
    if p and not fresh[id] then
      p.panes[p.at] = pane(p.session, p.block, p.panes[p.at].active, p.panes[p.at])
      fresh[id] = true
    end
  end
  return last
end

if not PADDOCK_WORKER then
  return snapshot((rex.args or {}).skip)
end
