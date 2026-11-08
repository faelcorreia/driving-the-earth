extends SceneTree

## Captura visual da cena M1 (V1). Uso:
##   godot --path godot -s tools/capture.gd -- --map casa-10km --times 17:45,18:10 --out <dir>
## Opcionais: --start 17:42 (hora local em que o relógio começa), --date 2026-11-24 (dia civil BRT
## em que começa; sem ele, o dia de hoje em BRT; o relógio sempre começa em --start), --settle 30 (s reais de espera
## com o mundo parado, antes do primeiro horário), --az 160 (ou lista 160,250: uma foto da
## câmera por azimute), --fast 60 (velocidade do avanço do relógio), --moon 1 (foto apontada
## para o disco da lua). Horário anterior ao --start é lido como do dia seguinte (ex.: 04:30).
## --point spawn|pole|high (padrão spawn): onde o carro fica. pole = sob a luz de poste acesa mais
## próxima do spawn; high = ponto mais alto da estrada do casa-10km (Crista Serra do Curral).
## Diagnóstico (opcionais, sem efeito quando ausentes): --exposure 6.0 (tonemap_exposure do
## WorldEnvironment), --pool 0 (lamp_real_lights do World: 0 desliga as luzes reais do poste),
## --top 1 (fotos de cima a TOP_HEIGHT_M: <HHMM>_top_car.png, _top_pole.png, _top_between.png;
## pole = luz acesa mais próxima do carro, near = 6 m à frente dela, between = 17,5 m à frente
## dela, na direção do carro). --car-under-pole 1 põe o carro sob essa luz antes das fotos de cima.
## --cloud-cover 0.5 (0 a 1, ausente = o padrão do Environment): sets the clock's cloud_cover.
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
const TOP_HEIGHT_M := 14.0
## --point high: ponto da estrada mais alta do casa-10km (bake: x leste, y norte, em m; cota 1379,6 m).
const SERRA_POINT_M := Vector2(778.8, -6614.4)
const SERRA_ALT_M := 1379.6
## Physics frames the car gets to drop onto the streamed road after a --point teleport.
const POINT_SETTLE_FRAMES := 600


func _initialize() -> void:
	run()


func _parse_args() -> Dictionary:
	var opts := {
		"map": "casa-10km",
		"times": "17:45,18:10,18:25,19:00",
		"out": "",
		"start": "17:42",
		"date": "",
		"settle": "30",
		"az": "160",
		"fast": "60",
		"moon": "0",
		"exposure": "",
		"pool": "",
		"top": "0",
		"car_under_pole": "0",
		"point": "spawn",
		"cloud_cover": "",
	}
	var args := OS.get_cmdline_user_args()
	var i := 0
	while i < args.size():
		var key := args[i].trim_prefix("--").replace("-", "_")
		if opts.has(key) and i + 1 < args.size():
			opts[key] = args[i + 1]
			i += 2
		else:
			i += 1
	return opts


## Lit pool light (omni or spot) nearest to `from`, or null when none is lit within reach.
func _nearest_pool_light(world: Node, from: Vector3) -> Node3D:
	var best: Node3D = null
	var best_d := INF
	for n in world.find_children("*", "", true, false):
		if (n is OmniLight3D or n is SpotLight3D) and (n as Node3D).is_visible_in_tree():
			var d := (n as Node3D).global_position.distance_squared_to(from)
			if d < best_d:
				best_d = d
				best = n
	return best


## Moves the car to a capture point (--point). "spawn" leaves it where the game put it. "pole" puts
## it under the lit pool light nearest to the spawn. "high" puts it on the highest road point of
## casa-10km, Crista Serra do Curral (bake frame: x east, y north, m from the map center; 1380 m).
func _apply_point(point: String, world: Node, car: Node3D) -> void:
	# Tiles stream in around the car and the body drops onto them: wait for both before the clock runs.
	print("capture: car at the game's spawn %s" % car.global_position)
	if point == "spawn":
		return
	var at := car.global_position
	if point == "pole":
		var pole := _nearest_pool_light(world, car.global_position)
		if pole == null:
			push_error("capture: no lit pool light for --point pole")
			quit(1)
			return
		at = Vector3(pole.global_position.x, car.global_position.y, pole.global_position.z)
	elif point == "high":
		at = Vector3(SERRA_POINT_M.x, SERRA_ALT_M + 2.0, -SERRA_POINT_M.y)
	else:
		push_error("capture: --point must be spawn, pole or high, got %s" % point)
		quit(2)
		return
	# The body's state lives in the physics server: a plain node move is undone at the next
	# physics sync, so teleport through the server and stop the car.
	var rid: RID = car.get_rid()
	PhysicsServer3D.body_set_state(rid, PhysicsServer3D.BODY_STATE_TRANSFORM, Transform3D(car.global_transform.basis, at))
	PhysicsServer3D.body_set_state(rid, PhysicsServer3D.BODY_STATE_LINEAR_VELOCITY, Vector3.ZERO)
	PhysicsServer3D.body_set_state(rid, PhysicsServer3D.BODY_STATE_ANGULAR_VELOCITY, Vector3.ZERO)
	for i in POINT_SETTLE_FRAMES:
		await physics_frame
	print("capture: point %s, car set at %s, settled at %s" % [point, at, car.global_position])


func _hm_to_min(hm: String) -> float:
	var parts := hm.split(":")
	return float(parts[0].to_int()) * 60.0 + float(parts[1].to_int())


func _min_to_hm(minutes: float) -> String:
	var t := int(round(minutes))
	return "%02d:%02d" % [t / 60, t % 60]


## Dia civil de hoje em Brasília (UTC-3, sem horário de verão desde 2019), YYYY-MM-DD.
func _today_brt() -> String:
	var d := Time.get_datetime_dict_from_unix_time(int(Time.get_unix_time_from_system()) - 3 * 3600)
	return "%04d-%02d-%02d" % [d["year"], d["month"], d["day"]]


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
	if opts["pool"] != "":
		world.set("lamp_real_lights", opts["pool"].to_int())
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
	if opts["cloud_cover"] != "":
		clock.set("cloud_cover", float(opts["cloud_cover"]))
	# O relógio começa às --start horas do dia pedido (--date) ou do dia de hoje (BRT). Sem isso,
	# o relógio ficaria na hora real do sistema e os horários do log não bateriam com o ambiente.
	var date: String = opts["date"] if opts["date"] != "" else _today_brt()
	var ymd: PackedStringArray = date.split("-")
	if ymd.size() != 3:
		push_error("capture: --date must be YYYY-MM-DD, got %s" % date)
		quit(2)
		return
	clock.call("set_local_time", ymd[0].to_int(), ymd[1].to_int(), ymd[2].to_int(), _hm_to_min(opts["start"]) / 60.0)
	print("capture: clock set to %s %s (BRT)" % [date, opts["start"]])

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
	var wenv := main.find_child("WorldEnvironment", true, false) as WorldEnvironment
	if opts["exposure"] != "" and wenv != null and wenv.environment != null:
		wenv.environment.tonemap_exposure = float(opts["exposure"])

	await _apply_point(opts["point"], world, car)

	var start_min := _hm_to_min(opts["start"])
	var fast := float(opts["fast"])
	var azimuths: Array = opts["az"].split(",", false)
	var moon_light := clock.find_child("Moon", false, false) as Node3D
	# O relógio do jogo (clock.rs) é a referência: a espera compara o instante dele com o alvo,
	# e o log lê a hora e a elevação do sol dele. Não há soma de deltas aqui.
	var start_unix: float = clock.call("game_unix")
	for hm: String in opts["times"].split(",", false):
		var target_min := _hm_to_min(hm)
		if target_min < start_min:
			target_min += 1440.0
		var target_unix := start_unix + (target_min - start_min) * 60.0
		if target_unix < float(clock.call("game_unix")):
			push_warning("capture: %s is behind the clock, skipped" % hm)
			continue
		clock.set("speed", fast)
		while float(clock.call("game_unix")) < target_unix:
			await process_frame
		clock.set("speed", 0.0)
		var actual: String = clock.call("local_time_text")
		var sun_alt: float = clock.call("sun_altitude_deg")
		# Direção da lua: o nó Moon aponta do observador para a lua (basis.z, ver clock.rs orient).
		var moon_dir := moon_light.global_transform.basis.z if moon_light != null else Vector3.FORWARD
		print("capture: target %s, clock at %s, sun alt %.2f deg, moon alt %.1f deg" % [hm, actual, sun_alt, rad_to_deg(asin(moon_dir.y))])

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

		# (b2) diagnóstico: de cima, a TOP_HEIGHT_M, sobre o carro, sobre a luz do poste mais
		# próxima e a meio caminho entre duas luzes acesas vizinhas.
		if opts["top"] == "1":
			# Between: half a lamp spacing (35 m, tileformat LAMP_SPACING_M) along the car's heading
			# from the nearest lit pool light; an estimate, the next pole is not known here.
			var pole := _nearest_pool_light(world, car.global_position)
			if pole != null and opts["car_under_pole"] == "1":
				# The body's state lives in the physics server: a plain node move is undone at the
				# next physics sync, so teleport through the server and stop the car.
				var at := Vector3(pole.global_position.x, car.global_position.y, pole.global_position.z)
				var rid: RID = car.get_rid()
				PhysicsServer3D.body_set_state(rid, PhysicsServer3D.BODY_STATE_TRANSFORM, Transform3D(car.global_transform.basis, at))
				PhysicsServer3D.body_set_state(rid, PhysicsServer3D.BODY_STATE_LINEAR_VELOCITY, Vector3.ZERO)
				PhysicsServer3D.body_set_state(rid, PhysicsServer3D.BODY_STATE_ANGULAR_VELOCITY, Vector3.ZERO)
				for i in 30:
					await physics_frame
			var centers := {"car": car.global_position}
			if pole != null:
				centers["pole"] = pole.global_position
				var ahead := -car.global_transform.basis.z
				ahead.y = 0.0
				ahead = ahead.normalized()
				centers["near"] = pole.global_position + ahead * 6.0
				centers["between"] = pole.global_position + ahead * 17.5
			for key in centers:
				var c: Vector3 = centers[key]
				car_cam.global_position = Vector3(c.x, car.global_position.y + TOP_HEIGHT_M, c.z)
				car_cam.look_at(Vector3(c.x, car.global_position.y, c.z), Vector3.FORWARD)
				for i in 5:
					await process_frame
				await _snap(out_dir.path_join("%s_top_%s.png" % [tag, key]))
				print("capture: top %s at %s, car body now at %s" % [key, c, car.global_position])

		# (c) a lua, se pedida: a câmera aponta para o disco (mesmo que esteja abaixo do horizonte).
		if opts["moon"] == "1" and moon_light != null:
			car_cam.global_position = eye
			car_cam.look_at(eye + moon_dir * 100.0, Vector3.UP)
			for i in 5:
				await process_frame
			await _snap(out_dir.path_join("%s_moon.png" % tag))

	quit(0)
