# A message starting with a bot's mention lists its commands (#3): none
# selected, Tab takes the first and Enter puts it in the box. Then
# Suggestions ▸ Slash Commands turns them off (its settings-proto 1 PATCH is
# in fake.log; FAKE_NSFW=dms=1 puts that proto in READY), and back on, and
# Enter with none selected sends the message.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type <@901> ro"
sleep 1
cmd "suggestions $OUT/bot-commands.png"
cmd "key Tab"
cmd "suggestions"
cmd "key Return"
cmd "line"
cmd "set "
sleep 1
cmd "composer-menu"
cmd "shot-popover composer $OUT/menu.png"
cmd "menu-item command-suggestions"
waitreq PATCH /api/v9/users/@me/settings-proto/1 1
cmd "type <@901> ro"
sleep 1
cmd "suggestions"
cmd "set "
cmd "composer-menu"
cmd "menu-item command-suggestions"
sleep 1
cmd "type <@901> st"
sleep 1
cmd "suggestions"
cmd "key Return"
waitreq POST /api/v9/channels/600/messages 1
cmd "line"
cmd "quit"
sleep 1
