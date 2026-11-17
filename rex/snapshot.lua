-- paddock's snapshot of a Rex server: every session, tab and terminal block,
-- as Rex reports them. src/backend/rex.rs turns it into paddock's model.
--
-- rex/worker.lua runs it for paddock. On its own it prints the snapshot as
-- JSON, for debugging:
--
--   rex do rex/snapshot.lua [skip=BLOCK_ID]
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

local SHELLS = { sh = true, bash = true, zsh = true, fish = true, dash = true, ksh = true, tcsh = true, csh = true, nu = true }

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

-- Blocks last read with a call unanswered (a connection's hiccup): what was
-- read of them may lack an agent's status, so it is not kept.
local shaky = {}

local function pane(session, block, active)
  local id = block.block_id
  local process = block_call(session, id, "process")
  local fg = (process or {}).foreground or {}
  local cwd = fg.cwd or (block_call(session, id, "pwd") or {}).pwd
  local st, answered = status(session, id, fg.pid)
  shaky[id] = (not process or not answered) or nil
  return {
    id = id,
    active = active,
    label = block.label,
    -- Only an agent's title is shown.
    title = st and (block_call(session, id, "title") or {}).title or nil,
    argv0 = fg.argv0 or fg.name,
    pid = fg.pid,
    cwd = cwd,
    status = st,
  }
end

-- The last snapshot, and where in it each pane came from, so that `refresh`
-- can read again only the blocks that changed; and the blocks read for the
-- request being answered, which need no second reading in it.
local last, listed
fresh = {}

-- Every session, tab and terminal block. A pane costs two to four calls
-- (its process, folder, status and title), a session one: with KEEP, the
-- panes the last snapshot has are kept as it read them (the listener names
-- the ones that change), with the label and focus this one finds, and only
-- new ones are read. Over a network that is the difference between a
-- snapshot in a tenth of a second and one that takes that for each pane.
function snapshot(skip, keep)
  local before = keep and listed or {}
  -- Only the views of attaches this snapshot makes are fresh.
  views = {}
  places = { blocks = {}, windows = {} }
  listed = {}
  local sessions = {}
  -- Failing, it says so: no sessions is not the same.
  local list, err = rex.call("session.list")
  if not list then error(tostring(err or "rex.call failed"), 0) end
  for _, s in ipairs(list.sessions or {}) do
    local id = on(s.session_id)
    local v = views[id] or rex.session.view { session_id = id } or {}
    views[id] = nil
    local windows = {}
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
            local had = before[b.block_id]
            local p = had and not shaky[b.block_id] and had.panes[had.at]
            if p then
              p.active, p.label = b.block_id == focused, b.label
            else
              p = pane(id, b, b.block_id == focused)
              fresh[b.block_id] = true
            end
            panes[#panes + 1] = p
            listed[b.block_id] = { session = id, block = b, panes = panes, at = #panes }
          end
        end
      end
      windows[#windows + 1] = { id = w.window_id, name = w.label, active = w.window_id == v.active_window_id, panes = panes }
    end
    sessions[#sessions + 1] = { id = id, name = s.label, windows = windows }
  end
  for id in pairs(shaky) do
    if not listed[id] then shaky[id] = nil end
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
      p.panes[p.at] = pane(p.session, p.block, p.panes[p.at].active)
      fresh[id] = true
    end
  end
  return last
end

if not PADDOCK_WORKER then
  return snapshot((rex.args or {}).skip)
end
