# The message box's @ list (#376): in a server, its members who can see the
# channel, then roles (Mods is mentionable) and, with FAKE_OWNER=1, @everyone
# and @here. Dice Roller, known only from a DM, isn't offered there; a pick
# of a role writes its mention.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type @"
sleep 1
cmd "suggestions $OUT/empty.png"
cmd "set "
cmd "type @d"
sleep 1
cmd "suggestions $OUT/d.png"
cmd "set "
cmd "type @e"
sleep 1
cmd "suggestions $OUT/e.png"
cmd "set "
cmd "type @mo"
sleep 1
cmd "suggestions"
cmd "pick 0"
cmd "line"
cmd "quit"
sleep 1
