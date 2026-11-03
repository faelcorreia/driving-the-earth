extends SceneTree

## Captura visual da cena M1 (V1). Uso:
##   godot --path godot -s tools/capture.gd -- --map casa-10km --times 17:45,18:10 --out <dir>
## Opcionais: --start 17:42 (hora local em que o relógio começa), --settle 30 (s reais de espera
## com o mundo parado, antes do primeiro horário), --az 160 (ou lista 160,250: uma foto da
## câmera por azimute), --fast 60 (velocidade do avanço do relógio), --moon 1 (foto apontada
## para o disco da lua). Horário anterior ao --start é lido como do dia seguinte (ex.: 04:30).
##
## Para cada horário: o relógio anda em tempo acelerado até o horário e para (speed 0). Então
## salva PNGs:
##   <HHMM>_spawn180.png: câmera de seguimento do carro (rumo do spawn).
##   <HHMM>_az<AZ>.png: só a câmera, parada no carro, olhando o azimute AZ (serra).
##   <HHMM>_moon.png: só a câmera, olhando para a direção do nó Moon (se --moon 1).
## O horário real do quadro é registrado pelo próprio script (game_s acumula delta x speed).

const SIZE := Vector2i(1280, 720)
const EYE_HEIGHT_M := 1.2
const SETTLE_FRAMES := 60


func _initialize() -> void:
	run()


func _parse_args() -> Dictionary:
	var opts := {
		"map": "casa-10km",
		"times": "17:45,18:10,18:25,19:00",
		"out": "",
		"start": "17:42",
		"settle": "30",
		"az": "160",
		"fast": "60",
		"moon": "0",
	}
	var args := OS.get_cmdline_user_args()
	var i := 0
	while i < args.size():
		var key := args[i].trim_prefix("--")
		if opts.has(key) and i + 1 < args.size():
			opts[key] = args[i + 1]
			i += 2
		else:
			i += 1
	return opts


func _hm_to_min(hm: String) -> float:
	var parts := hm.split(":")
	return float(parts[0].to_int()) * 60.0 + float(parts[1].to_int())


func _min_to_hm(minutes: float) -> String:
	var t := int(round(minutes))
	return "%02d:%02d" % [t / 60, t % 60]


func _snap(path: String) -> void:
	await RenderingServer.frame_post_draw
	var err := root.get_texture().get_image().save_png(path)
	if err != OK:
		push_error("capture: cannot save %s (error %d)" % [path, err])
	else:
		print("capture: saved ", path)


func run() -> void:
	var opts := _parse_args()
	var out_dir: String = opts["out"]
	if out_dir == "":
		push_error("capture: --out is required")
		quit(2)
		return
	DirAccess.make_dir_recursive_absolute(out_dir)
	DisplayServer.window_set_size(SIZE)
	root.size = SIZE

	var main: Node = (load("res://main.tscn") as PackedScene).instantiate()
	var world := main.get_node("World")
	# map_id vai no nó antes de entrar na árvore, então o mundo nasce com o mapa certo.
	world.set("map_id", opts["map"])
	root.add_child(main)

	# Nodes added during the first frames are set up at the next frame, so the clock shows up later.
	var clock: Node = null
	for i in 120:
		await process_frame
		clock = world.find_child("Environment", true, false)
		if clock != null:
			break
	if clock == null:
		push_error("capture: no Environment clock under World")
		quit(1)
		return
	# Relógio parado até o primeiro horário; o mundo carrega em tempo real.
	clock.set("speed", 0.0)

	var waited := 0.0
	var settle := float(opts["settle"])
	while waited < settle:
		await process_frame
		waited += root.get_process_delta_time()

	var car_cam := world.find_child("Camera", true, false) as Camera3D
	var driver := world.find_child("DriverCamera", true, false) as Camera3D
	if car_cam == null or driver == null:
		push_error("capture: car cameras not found (spawn failed?)")
		quit(1)
		return
	var car := car_cam.get_parent() as Node3D

	var start_min := _hm_to_min(opts["start"])
	var fast := float(opts["fast"])
	var azimuths: Array = opts["az"].split(",", false)
	var moon_light := clock.find_child("Moon", false, false) as Node3D
	var game_s := 0.0
	for hm: String in opts["times"].split(",", false):
		var target_min := _hm_to_min(hm)
		if target_min < start_min:
			target_min += 1440.0
		var target_s := (target_min - start_min) * 60.0
		if target_s < game_s:
			push_warning("capture: %s is behind the clock, skipped" % hm)
			continue
		clock.set("speed", fast)
		while game_s < target_s:
			await process_frame
			game_s += root.get_process_delta_time() * fast
		clock.set("speed", 0.0)
		var actual := _min_to_hm(start_min + game_s / 60.0)
		# Direção da lua: o nó Moon aponta do observador para a lua (basis.z, ver clock.rs orient).
		var moon_dir := moon_light.global_transform.basis.z if moon_light != null else Vector3.FORWARD
		print("capture: target %s, clock at %s, moon alt %.1f deg" % [hm, actual, rad_to_deg(asin(moon_dir.y))])

		var tag := hm.replace(":", "")

		# (a) câmera de seguimento do carro, olhando o rumo do spawn.
		car_cam.call("set_active", true)
		driver.call("set_active", false)
		for i in SETTLE_FRAMES:
			await process_frame
		await _snap(out_dir.path_join("%s_spawn180.png" % tag))

		# (b) só a câmera, na altura dos olhos, olhando cada azimute pedido (serra).
		car_cam.call("set_active", false)
		car_cam.current = true
		var eye := car.global_position + Vector3.UP * EYE_HEIGHT_M
		for az: String in azimuths:
			var h := deg_to_rad(az.to_float())
			var dir := Vector3(sin(h), 0.0, -cos(h))
			car_cam.global_position = eye
			car_cam.look_at(eye + dir * 100.0, Vector3.UP)
			for i in 5:
				await process_frame
			print("capture: shooting az %s" % az)
			await _snap(out_dir.path_join("%s_az%s.png" % [tag, az]))
			print("capture: az %s done" % az)

		# (c) a lua, se pedida: a câmera aponta para o disco (mesmo que esteja abaixo do horizonte).
		if opts["moon"] == "1" and moon_light != null:
			car_cam.global_position = eye
			car_cam.look_at(eye + moon_dir * 100.0, Vector3.UP)
			for i in 5:
				await process_frame
			await _snap(out_dir.path_join("%s_moon.png" % tag))

	quit(0)
