-- paddock for Rex: an action that opens paddock in a floating pane over the
-- current window. `paddock init --rex` installs a copy made for that paddock
-- beside Rex's init.lua, and prints how to load it; or load this one from a
-- checkout. In ~/.config/rex/init.lua:
--
--   local paddock = dofile("/path/to/paddock.lua")
--   paddock.setup{ key = "cmd+shift+a" }
--
-- Options (all optional):
--   key      key to bind, such as "cmd+shift+a"; nil binds nothing. The
--            key toggles: pressed again, it closes paddock
--   bin      the paddock binary: a path, or a name found on the PATH
--            (default: the one `paddock init --rex` made this copy for,
--            else "paddock"). A path starting "~/" is in the home directory
--            of the machine paddock runs on, which for a remote host's
--            session is that host; missing there, paddock on its PATH runs
--            instead
--   tab      "sessions" or "agents"; nil opens on the tab paddock was last
--            closed on
--   bounds   { x, y, w, h } as fractions of the window (default: centred)
--   colors   paddock's colours by role: accent, muted, hint, border, idle,
--            finished, working, asking. Values as tmux writes colours:
--            "#a2a970", "red", "brightblue", "colour214"
--   icons    "nerd" for Nerd Font agent marks (default "unicode")
--   suggest  a shell command that prints a title for an agent's pane, given
--            the Rex block ID as its argument (the rename's suggestion)
--   theme    the floating terminal's own colours:
--            { foreground=, background=, cursor=, palette={ ... 16 } }

local M = {}

-- Filled in by `paddock init --rex`: the binary this copy was made for, and
-- its version, which paddock checks against its own.
local BIN = nil
local VERSION = nil

local LABEL = "paddock"
local FLAVOR = "com.superlogical.terminal.shell"

-- RESULT, else ERR raised: Rex's calls answer nil and an error, which an
-- action that returned them would drop, leaving the key to do nothing.
local function check(result, err)
  if result == nil and err ~= nil then error(tostring(err), 0) end
  return result
end

-- The floating layer of WINDOW in SESSION that holds paddock, if any.
local function find_layer(session, window)
  local view = rex.session.view { session_id = session }
  for _, w in ipairs(view and view.windows or {}) do
    if w.window_id == window then
      for _, layer in ipairs(w.layers or {}) do
        for _, b in ipairs(layer.kind == "floating" and layer.blocks or {}) do
          if b.label == LABEL and b.flavor == FLAVOR then return layer.layer_id end
        end
      end
    end
  end
end

-- Run BIN with ARGS on the machine the layer runs on: "~/" is its home, a
-- bare name is looked up on its PATH, and a BIN missing there falls back to
-- paddock on its PATH. VERSION goes with a BIN given as a path, as
-- PADDOCK_PLUGIN_VERSION, but not with one found on the PATH: a host's own
-- paddock need not be this copy's.
local function command(bin, args)
  local run = { "/bin/sh", "-c", 'b=$1 v=$2; shift 2; case $b in "~/"*) b=$HOME/${b#"~/"} ;; esac; '
    .. 'case $b in */*) [ -x "$b" ] || { b=paddock; v=; } ;; *) v=; command -v "$b" >/dev/null 2>&1 || b=paddock ;; esac; '
    .. '[ -z "$v" ] || export PADDOCK_PLUGIN_VERSION="$v"; exec "$b" "$@"', "paddock", bin, VERSION or "" }
  for _, v in ipairs(args) do run[#run + 1] = v end
  return run
end

local function open(opts, ctx, args)
  args = args or {}
  local session = ctx.session_id
  local window
  if ctx.block_id then
    local d = rex.session.describe_block { session_id = session, block_id = ctx.block_id }
    window = d and d.block and d.block.window_id
  end
  if not window then
    local view = rex.session.view { session_id = session }
    window = view and view.active_window_id
  end
  -- Open already: the key closes it.
  local open_layer = window and find_layer(session, window)
  if open_layer then
    return check(rex.session.close_layer { session_id = session, layer_id = open_layer })
  end
  local argv = { "--rex", "--popup" }
  local function add(...)
    for _, v in ipairs { ... } do argv[#argv + 1] = v end
  end
  local tab = args.tab or opts.tab
  if tab then add("--tab", tab) end
  if ctx.block_id then add("--from", ctx.block_id) end
  -- The app by its label where it has one: paddock may run on a remote
  -- host (for that host's sessions), where the app has another ID.
  local client = args.client
  if not client and ctx.client_id then
    local ok, found = pcall(rex.call, "client.inspect", { client_id = ctx.client_id })
    found = ok and found
    local label = found and found.client and found.client.info and found.client.info.label
    client = (label and label ~= "") and label or ctx.client_id
  end
  if client then add("--client", client) end
  for role, colour in pairs(opts.colors or {}) do add("--color", role .. "=" .. colour) end
  if opts.icons then add("--icons", opts.icons) end
  if opts.suggest then add("--suggest", opts.suggest) end
  local options = { command = command(opts.bin or BIN or "paddock", argv) }
  if opts.theme then options.theme = opts.theme end
  return check(rex.session.new_layer {
    session_id = session,
    window_id = window,
    bounds = opts.bounds or { 0.1, 0.08, 0.8, 0.84 },
    focus = args.focus ~= false,
    layout = { block = { flavor = FLAVOR, label = LABEL, options = options } },
  })
end

function M.setup(opts)
  opts = opts or {}
  rex.action {
    name = "paddock",
    title = "paddock: Sessions and Agents",
    description = "Fuzzy-find sessions, tabs and panes, and jump to the agent that needs you.",
    keywords = "agents fuzzy finder switch jump",
    args = { tab = "string?", client = "string?", focus = "boolean?" },
    run = function(ctx, args) return open(opts, ctx, args) end,
  }
  if opts.key and rex.bind then
    rex.bind(opts.key, "paddock")
  end
end

return M
