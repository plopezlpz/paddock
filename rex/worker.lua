-- paddock's worker on a Rex server: one `rex do` for the popup's lifetime,
-- run by src/backend/rex.rs after rex/snapshot.lua (its `snapshot`, `on`,
-- `block_call` and `places`). It reads one JSON request per line on stdin
-- and answers each with one JSON line on stdout, {"ok":VALUE} or
-- {"err":"MESSAGE"}, so paddock makes its Rex calls over one connection.
--
-- PADDOCK_ME and PADDOCK_HERE are paddock's own block and session;
-- PADDOCK_FROM is the block focused when paddock opened (snapshot.lua).

local FLAVOR = "com.superlogical.terminal.shell"

-- A call's result, or its error raised as the request's answer, without the
-- transport's prefix (stream response "control-9" failed: …).
local function check(result, err)
  if result == nil then
    error((tostring(err or "the call failed"):gsub('^stream response "[^"]*" failed: ', "")), 0)
  end
  return result
end

local function given(v)
  return v ~= nil and v ~= ""
end

-- Where a block or tab is, from the last snapshot, else from the sessions
-- and tabs as they are now.
local function place(kind, id, gone)
  if not places[kind][id] then snapshot(PADDOCK_ME, true) end
  return places[kind][id] or error(gone, 0)
end

local function block_session(id)
  return place("blocks", id, "the pane is gone").session
end

local function window_session(id)
  return place("windows", id, "the tab is gone")
end

local function window_order(session)
  local ids = {}
  for _, w in ipairs(check(rex.session.view { session_id = on(session) }).windows or {}) do ids[#ids + 1] = w.window_id end
  return ids
end

local function index_of(list, value)
  for i, v in ipairs(list) do
    if v == value then return i end
  end
end

-- Move window ID just before window BEFORE, or to the end.
local function move_before(session, id, before)
  check(rex.session.move_window { session_id = on(session), window_id = id, before_window_id = before })
end

-- Pane moves take focus with them: give it back to paddock.
local function refocus()
  if given(PADDOCK_ME) then rex.session.focus_block { session_id = on(PADDOCK_HERE), block_id = PADDOCK_ME } end
end

local function shell(dir)
  return { block = { flavor = FLAVOR, options = given(dir) and { cwd = dir } or {} } }
end

local ops = {}

-- A snapshot: everything read (a whole one); or with `layout`, the sessions
-- and tabs read again and the panes kept as last read, but for new ones; or
-- the last one. Then BLOCKS, if any, are read again.
function ops.snapshot(req)
  fresh = {}
  if req.layout then
    snapshot(req.skip, true)
  elseif not req.blocks then
    return snapshot(req.skip)
  end
  return refresh(req.skip, req.blocks or {})
end

-- The other Rex servers the app connects to (`rex hosts`).
function ops.hosts()
  return rex.servers()
end

-- CLIENT's label: the app is a client of each host too, under that label.
function ops.client_label(req)
  local c = check(rex.call("client.inspect", { client_id = req.client })).client or {}
  return (c.info or {}).label or ""
end

-- The window holding BLOCK.
function ops.where(req)
  local d = check(rex.session.describe_block { session_id = on(PADDOCK_HERE), block_id = req.block })
  return d.block and d.block.window_id or ""
end

-- Result ID's text from byte OFFSET to its end. max_bytes counts the data
-- JSON-escaped (an ESC is 6 bytes), so a read can stop short: read on to eof.
local READ = 983040
local function read_to_end(session, block, id, offset)
  local part, err
  -- An offset inside a character is refused: step to the next byte.
  for _ = 1, 4 do
    part, err = block_call(session, block, "blob_read", { id = id, offset = offset, max_bytes = READ })
    if part then break end
    offset = offset + 1
  end
  part = check(part, err)
  local chunks = { part.data or "" }
  while not part.eof and #(part.data or "") > 0 do
    offset = offset + #part.data
    part = check(block_call(session, block, "blob_read", { id = id, offset = offset, max_bytes = READ }))
    chunks[#chunks + 1] = part.data or ""
  end
  return table.concat(chunks)
end

-- The end of a block's screen: {rows = its height, text = at least that many
-- lines}; paddock keeps the last ROWS. Rex's vt format holds the whole
-- scrollback and builds it all on each call, so format is asked only for its
-- size (max_bytes = 1) and the end is read on its own, from 32 KiB back,
-- further while that holds less than a screenful.
local TAIL = 32768
function ops.capture(req)
  local session = block_session(req.block)
  local rows = (block_call(session, req.block, "size") or {}).rows or 60
  local r = check(block_call(session, req.block, "format", { format = "vt", max_bytes = 1 }))
  local text = r.content or ""
  if r.id and r.total then
    local want = TAIL
    while true do
      local offset = math.max(0, r.total - want)
      text = read_to_end(session, req.block, r.id, offset)
      -- More than ROWS lines? (gsub would count them all, slowly.)
      local at = 0
      for _ = 0, rows do at = at and text:find("\n", at + 1, true) end
      if at or offset == 0 then break end
      want = want * 4
    end
  end
  return { rows = rows, text = text }
end

function ops.rename_session(req)
  check(rex.session.set_label { session_id = on(req.session), label = req.name })
end

function ops.rename_window(req)
  check(rex.session.set_window_label { session_id = on(window_session(req.window)), window_id = req.window, label = req.name })
end

-- A pane's label, which paddock shows as its agent's title. An empty
-- label goes back to Rex's default, which paddock takes as no title.
function ops.rename_pane(req)
  check(rex.session.set_block_label { session_id = on(block_session(req.block)), block_id = req.block, label = req.name })
end

function ops.kill_session(req)
  check(rex.call("session.destroy", { session_id = on(req.session) }))
end

function ops.kill_window(req)
  check(rex.session.close_window { session_id = on(req.session), window_id = req.window })
end

function ops.kill_pane(req)
  check(rex.call("block.close", { session_id = on(block_session(req.block)), block_id = req.block }))
end

function ops.swap_windows(req)
  local ids = window_order(req.session)
  local ia, ib = index_of(ids, req.a), index_of(ids, req.b)
  if not (ia and ib) then error("the tab is gone", 0) end
  local a, b = req.a, req.b
  if ia > ib then a, b, ia, ib = b, a, ib, ia end
  local after_a = ids[ia + 1]
  -- A takes B's place; B then takes A's, unless they were side by side.
  move_before(req.session, a, ids[ib + 1])
  if after_a ~= b then move_before(req.session, b, after_a) end
end

function ops.swap_panes(req)
  local session = block_session(req.a)
  if session ~= block_session(req.b) then error("Rex can't swap panes between sessions", 0) end
  check(rex.session.swap_blocks { session_id = on(session), block_id = req.a, other_block_id = req.b })
  refocus()
end

-- Move WINDOW beside window TO (before it, or after), in the same session.
function ops.move_window(req)
  if req.from ~= req.into then error("Rex can't move a tab to another session", 0) end
  local ids = window_order(req.from)
  table.remove(ids, index_of(ids, req.window) or error("the tab is gone", 0))
  local at = index_of(ids, req.to) or error("the tab is gone", 0)
  move_before(req.from, req.window, req.before and req.to or ids[at + 1])
end

-- Move PANE into a split beside pane TO: side by side when TO is wide.
function ops.join_pane(req)
  local session = block_session(req.to)
  if session ~= block_session(req.pane) then error("Rex can't move a pane to another session", 0) end
  local size = block_call(session, req.to, "size") or {}
  local wide = (size.columns or 0) >= 2 * (size.rows or 0)
  check(rex.session.move_block {
    session_id = on(session),
    block_id = req.pane,
    anchor_block_id = req.to,
    direction = wide and "horizontal" or "vertical",
    side = req.before and "before" or "after",
  })
  refocus()
end

-- Make PANE a tab of its own at the end of SESSION. Rex places an existing
-- block only beside another: a new tab's shell, closed once it is moved.
function ops.break_pane(req)
  local session = block_session(req.pane)
  if session ~= req.session then error("Rex can't move a pane to another session", 0) end
  local made = check(rex.session.new_window { session_id = on(session), focus = false, layout = shell() })
  local placeholder = (made.block_ids or {})[1] or error("Rex made no tab for the pane", 0)
  local moved, err = pcall(function()
    check(rex.session.move_block { session_id = session, block_id = req.pane, anchor_block_id = placeholder })
  end)
  -- Moved or not, the placeholder goes: a failed move leaves no stray tab.
  rex.call("block.close", { session_id = session, block_id = placeholder })
  refocus()
  if not moved then error(err, 0) end
end

function ops.new_session(req)
  local made = check(rex.call("session.create", {
    label = given(req.name) and req.name or nil,
    initial_windows = { { layout = shell(req.dir) } },
  }))
  return made.session_id or ""
end

-- A tab after window AFTER, named NAME (or by Rex), in folder DIR.
function ops.new_window(req)
  local made = check(rex.session.new_window {
    session_id = on(req.session),
    focus = false,
    window_label = given(req.name) and req.name or nil,
    layout = shell(req.dir),
  })
  -- Placed after AFTER if it can be; else it stays at the end, and is made.
  pcall(function()
    local ids = window_order(req.session)
    -- (Without an index, table.remove takes the last.)
    table.remove(ids, index_of(ids, made.window_id) or error("not listed yet"))
    local at = index_of(ids, req.after)
    if at and ids[at + 1] then move_before(req.session, made.window_id, ids[at + 1]) end
  end)
  return made.window_id
end

-- Focus the target, then ask the app to show its session. Showing a session
-- other than paddock's own needs Remote Control on in Rex.app.
function ops.jump(req)
  if given(req.block) then
    check(rex.session.focus_block { session_id = on(req.session), block_id = req.block })
  elseif given(req.window) then
    check(rex.session.focus_window { session_id = on(req.session), window_id = req.window })
  end
  local select = { session_id = req.session }
  if given(req.window) then select.window_id = req.window end
  local ok, err = pcall(rex.client.queue, "session.select", select)
  if not ok and req.session ~= PADDOCK_HERE then
    err = tostring(err)
    if err:find("not accepting actions", 1, true) then
      error("can't switch sessions: turn on Remote Control in Rex Settings > Rex Server", 0)
    end
    error("can't switch sessions: " .. err:gsub("^[^:]*:%d+: ", ""), 0)
  end
end

for line in io.lines() do
  local ok, result = pcall(function()
    local req = check(rex.json.decode(line))
    local op = ops[req.op] or error("unknown request " .. tostring(req.op), 0)
    return check(rex.json.encode { ok = op(req) or "" })
  end)
  io.write(ok and result or (rex.json.encode { err = tostring(result) } or '{"err":"unreadable error"}'), "\n")
  io.flush()
end
