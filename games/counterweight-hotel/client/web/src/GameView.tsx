import { useEffect, useRef, useState } from "react";
import type { PointerEvent } from "react";
import { createBridgePosePublisher } from "./bridgePresence";
import { DocumentArtwork, SealImprint } from "./DocumentArtwork";
import { eastStoresFloor, type Action, type Observation, type Seal } from "./game";

export interface Snapshot {
  observation: Observation;
  interactionEnabled: boolean;
  transition?: { action: Action } | null;
  act(action: Action): void;
}

const seals: Record<Seal, string> = { stag: "Stag", rose: "Rose", key: "Key" };
const sealChoices: Seal[] = ["stag", "rose", "key"];

const panels = {
  ballast: "Suitcase rack",
  partner: "Exchange hatch",
  key: "Brass brake key",
  switch: "Key switch",
  brake: "Brake pedestal",
  plate: "Funicular works · 1896",
  exit: "Bridge inspection lock",
  "route-card": "Rescue bridge route",
  bulletin: "House bulletin",
} as const;

type Panel = keyof typeof panels;

const walkDirections = {
  forward: { x: 0, forward: 1, icon: "↑" },
  left: { x: -1, forward: 0, icon: "←" },
  backward: { x: 0, forward: -1, icon: "↓" },
  right: { x: 1, forward: 0, icon: "→" }
} as const;

type WalkDirection = keyof typeof walkDirections;

/** Formats the ground landing consistently across the lift indicator and route sign. */
function floorName(floor: number) {
  return floor === 0 ? "Ground" : `Floor ${floor}`;
}

/** Capitalizes the wing name used on signs and on the route sign. */
function wingName(wing: Observation["wing"]) {
  return wing === "east" ? "East" : "West";
}

const viewSettingsKey = "counterweight-hotel.view";
const viewingAngles = [50, 60, 75] as const;

/** Keeps viewing preferences local to this browser, including during replays and reconnects. */
function readViewSettings() {
  const defaults = { reduced: true, animateLift: true, degrees: 60 };
  try {
    const saved = JSON.parse(localStorage.getItem(viewSettingsKey) ?? "null");
    return {
      reduced: typeof saved?.reduced === "boolean" ? saved.reduced as boolean : defaults.reduced,
      animateLift: saved?.animateLift !== false,
      degrees: viewingAngles.some(angle => angle === saved?.degrees) ? saved.degrees as number : defaults.degrees,
    };
  } catch {
    return defaults;
  }
}

/** Binds the scenic prop targets and accessible dock to server-authorized actions. */
export function GameView({ snapshot }: { snapshot: Snapshot }) {
  const [viewSettings, setViewSettings] = useState(readViewSettings);
  const [viewOpen, setViewOpen] = useState(false);
  const viewButton = useRef<HTMLButtonElement>(null);
  const [panel, setPanel] = useState<Panel | null>(null);
  const [sceneState, setSceneState] = useState<"loading" | "ready" | "unavailable">("loading");
  const [hovered, setHovered] = useState<string | null>(null);
  const [revealObjects, setRevealObjects] = useState(false);
  const [atDoor, setAtDoor] = useState(false);
  const canvas = useRef<HTMLCanvasElement>(null);
  const instrument = useRef<HTMLElement>(null);
  const panelButton = useRef<HTMLButtonElement | null>(null);
  const scene = useRef<ReturnType<typeof import("./scene/hotel").createHotelScene> | null>(null);
  const heldPointers = useRef(new Map<number, WalkDirection>());
  const activatedDirection = useRef<WalkDirection | null>(null);
  const activationTimer = useRef<number | undefined>(undefined);
  const { observation, interactionEnabled } = snapshot;
  const wing = wingName(observation.wing);
  const otherWing = observation.wing === "east" ? "West" : "East";
  const arrived = observation.handed_over && observation.partner_floor === observation.partner_destination;
  const previousHandoff = useRef(observation.handed_over);
  const [inTransit, setTraveling] = useState(false);
  const [landedFloor, setLandedFloor] = useState(observation.floor);
  // A remote operator may release again before this browser finishes the first stroke.
  const displayedFloor = sceneState === "ready" ? landedFloor : observation.floor;
  const displayedPartnerFloor = observation.floor + observation.partner_floor - displayedFloor;
  const traveling = inTransit || displayedFloor !== observation.floor;
  const atRescueBridge = arrived && !traveling;
  const [rewinding, setRewinding] = useState(false);
  const roomState = {
    room: observation.wing === "east" ? "warm" : "cool",
    floor: observation.floor,
    partnerFloor: observation.partner_floor,
    onLanding: observation.on_landing,
    canStepOntoLanding: observation.actions.some(action => action.type === "step_onto_landing"),
    gateOpen: observation.gate_open,
    partnerGateOpen: observation.partner_gate_open,
    bridgePowered: observation.handed_over,
    partnerEscaped: observation.partner_exited,
    partnerBridgePose: observation.partner_bridge_pose ?? null,
    freightClear: observation.freight_status === null ? null : observation.freight_status === "ready",
    freightStatus: observation.freight_status,
    freightFloor: observation.freight_floor,
    canCollectFreight: !traveling && observation.actions.some(action => action.type === "collect_rescue_case"),
    lockTripped: observation.lock_tripped,
    partnerLockTripped: observation.partner_lock_tripped,
    canUnloadCase: !traveling && observation.actions.some(action => action.type === "unload"),
    loadableRackFloors: traveling ? [] : observation.actions.flatMap(action => action.type === "load_rack" ? [action.floor] : []),
    hasBrakeKey: observation.handle === observation.wing,
    loads: observation.cargo,
    partnerLoads: observation.partner_cargo,
    route: observation.route,
    atRescueBridge: arrived,
    landingLoads: observation.landing_loads,
    partnerLandingLoads: observation.partner_landing_loads,
    partnerDestination: observation.partner_destination,
    partnerSeal: observation.partner_seal,
    partnerDelivery: observation.partner_delivery,
    escaped: observation.exited
  } as const;

  /** Checks the game catalogue and the participant connection before an instrument can act. */
  function can(type: Action["type"], seal?: Seal) {
    return interactionEnabled && !traveling && !rewinding && observation.actions.some((action) =>
      action.type === type && (seal === undefined || (action.type === "exit" && action.seal === seal)));
  }

  const activePanel = observation.gate_open || observation.on_landing ? null : panel;
  const direction = Math.sign(observation.partner_cargo - observation.cargo);
  const balance = direction === 0 ? "Equal loads · cable balanced" : `${direction > 0 ? otherWing : wing} heavier · brake holding`;
  const freightReading = observation.freight_status === "awaiting_pickup" ? "Dispensary suitcase at East stores"
    : observation.freight_status === "aboard" ? "Dispensary suitcase aboard · manifest weights outstanding"
    : "Dispensary suitcase aboard · manifest weights cleared";
  const gatePower = observation.handed_over ? "Power on" : "No power";
  const gangwayReading = atRescueBridge ? "Gangway docked" : "Gangway retracted";
  /** Describes each gate's own inspection spring and latch independently of supply and docking. */
  function gateReading(open: boolean, tripped: boolean) {
    if (open) return "Unlocked · seal accepted";
    if (tripped) return "Locked · inspection spring tripped";
    return "Locked · seal pending";
  }
  const brakeNotice = observation.partner_gate_open ? "The other gate is open. The cable is secured."
    : observation.partner_on_landing ? `The ${otherWing} terrace interlock is engaged.`
    : observation.handle !== observation.wing ? "The key socket is empty. The lever is locked."
    : direction === 0 ? "The cable is balanced."
    : "The cable is against its end stop.";
  const hatchNotice = observation.floor !== observation.partner_floor ? "The other hatch is out of sight."
    : "The two hatches are aligned.";
  const objectLabels: Record<string, string> = {
    reach_refuge: "enter the refuge",
    bulletin: "read the house bulletin",
    step_onto_landing: "step onto the terrace", board_lift: "return to the room",
    collect_rescue_case: "load the dispensary suitcase", load: "inspect the suitcase rack", unload: "remove this suitcase", "route-card": "read the " + otherWing + " Room’s rescue bridge route",
    door: "inspect the bridge gate", brake: can("reset_lock") ? "reset the inspection lock" : can("release") ? "pull the brake lever" : "inspect the brake",
    "brake key": "inspect the brass brake key", "brake key switch": "inspect the brake key switch",
    partner: "inspect the exchange hatch", plate: "read the workshop plate"
  };
  const hoverLabel = hovered?.startsWith("load:") ? "bring this suitcase aboard" : hovered ? objectLabels[hovered] : null;
  const walkingEnabled = interactionEnabled && !activePanel && !viewOpen;

  /** Sends one vector from the unique directions active on the movement pad. */
  function syncWalkVector() {
    const directions = new Set(heldPointers.current.values());
    if (activatedDirection.current) directions.add(activatedDirection.current);
    let x = 0;
    let forward = 0;
    for (const direction of directions) {
      x += walkDirections[direction].x;
      forward += walkDirections[direction].forward;
    }
    scene.current?.setWalkVector(x, forward);
  }

  /** Stops movement when input is interrupted or control of the room is suspended. */
  function clearWalking() {
    heldPointers.current.clear();
    window.clearTimeout(activationTimer.current);
    activatedDirection.current = null;
    syncWalkVector();
  }

  /** Takes a short step for keyboard and assistive technology activations of a pad button. */
  function step(direction: WalkDirection) {
    if (!walkingEnabled) return;
    window.clearTimeout(activationTimer.current);
    activatedDirection.current = direction;
    syncWalkVector();
    activationTimer.current = window.setTimeout(() => {
      activatedDirection.current = null;
      syncWalkVector();
    }, 160);
  }

  /** Captures each movement pointer so walking continues while the direction is held. */
  function holdDirection(event: PointerEvent<HTMLButtonElement>, direction: WalkDirection) {
    if (event.button !== 0 || !walkingEnabled) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    heldPointers.current.set(event.pointerId, direction);
    syncWalkVector();
  }

  /** Releases the direction held by an individual pointer without affecting other touches. */
  function releaseDirection(event: PointerEvent<HTMLButtonElement>) {
    if (heldPointers.current.delete(event.pointerId)) syncWalkVector();
  }

  /** Submits only actions offered by the current observation while interaction is enabled. */
  function act(action: Action) {
    if (can(action.type, action.type === "exit" ? action.seal : undefined)) snapshot.act(action);
  }

  /** Gives the tripped inspection spring time to rewind at the pedestal before it can be used again. */
  useEffect(() => {
    if (!rewinding) return;
    if (!interactionEnabled || !observation.lock_tripped || observation.on_landing) {
      setRewinding(false);
      return;
    }
    const timer = window.setTimeout(() => {
      snapshot.act({ type: "reset_lock" });
      setRewinding(false);
    }, 3200);
    return () => window.clearTimeout(timer);
  }, [rewinding, interactionEnabled, observation.lock_tripped, observation.on_landing]);

  /** Opens a different instrument panel or dismisses the current one. */
  function togglePanel(next: Panel, trigger?: HTMLButtonElement) {
    setViewOpen(false);
    panelButton.current = trigger ?? null;
    setPanel((current) => current === next ? null : next);
  }

  /** Closes the active instrument and returns keyboard focus to its dock control. */
  function closePanel() {
    setPanel(null);
    const trigger = panelButton.current;
    if (trigger) requestAnimationFrame(() => trigger.focus({ preventScroll: true }));
  }

  /** Maps in-world instruments to the same handlers used by the keyboard-accessible dock. */
  function pick(target: string) {
    if (target === "reach_refuge") {
      act({ type: target });
      return;
    }
    if (!walkingEnabled) return;
    if (target === "step_onto_landing" || target === "board_lift") {
      act({ type: target });
      return;
    }
    if (observation.on_landing || observation.gate_open) return;
    if (target === "unload" || target === "collect_rescue_case") act({ type: target });
    if (target.startsWith("load:")) {
      const floor = Number(target.slice(5));
      const action = observation.actions.find(a => a.type === "load_rack" && a.floor === floor);
      if (action) act(action);
    }
    if (target === "brake") {
      if (can("reset_lock")) setRewinding(true);
      else if (can("release")) act({ type: "release" });
      else togglePanel("brake");
    }
    if (target === "key" || target === "switch" || target === "partner" || target === "plate") togglePanel(target);
    if (target === "load") togglePanel("ballast");
    if (target === "route-card" || target === "bulletin") togglePanel(target);
    if (target === "door") togglePanel("exit");
  }

  const latest = useRef({ pick, roomState, walkingEnabled, viewSettings, snapshot, traveling });
  latest.current = { pick, roomState, walkingEnabled, viewSettings, snapshot, traveling };

  /** Loads Babylon only for active play and releases graphics resources on exit. */
  useEffect(() => {
    let active = true;
    void import("./scene/hotel").then(({ createHotelScene }) => {
      if (!active || !canvas.current) return;
      const publish = createBridgePosePublisher(action => latest.current.snapshot.act(action));
      scene.current = createHotelScene(canvas.current, target => latest.current.pick(target), setHovered,
        setTraveling, setLandedFloor, pose => {
          const { snapshot: current, traveling } = latest.current;
          publish(pose, current.interactionEnabled && current.observation.gate_open && !current.observation.exited && !traveling);
        }, setAtDoor);
      scene.current.setMotionPreferences(latest.current.viewSettings.reduced, latest.current.viewSettings.animateLift);
      scene.current.setVerticalFov(latest.current.viewSettings.degrees);
      scene.current.update(latest.current.roomState);
      scene.current.setWalkingEnabled(latest.current.walkingEnabled && !document.hidden && document.hasFocus());
      setSceneState("ready");
    }).catch(() => {
      if (active) setSceneState("unavailable");
    });
    return () => {
      active = false;
      scene.current?.dispose();
      scene.current = null;
    };
  }, []);

  /** Apply changes immediately even if browser storage is unavailable. */
  useEffect(() => {
    scene.current?.setMotionPreferences(viewSettings.reduced, viewSettings.animateLift);
    scene.current?.setVerticalFov(viewSettings.degrees);
    try {
      localStorage.setItem(viewSettingsKey, JSON.stringify(viewSettings));
    } catch { /* The current tab still owns its preferences. */ }
  }, [viewSettings]);

  /** Refreshes the lift and re-enables physical suitcase targets when a visual stroke settles. */
  useEffect(() => {
    scene.current?.update(roomState);
  }, [observation, traveling]);

  /** Reveal the route and the moving key once the server accepts the exchange. */
  useEffect(() => {
    if (observation.handed_over && !previousHandoff.current && (panel === "partner" || panel === "key" || panel === "switch")) closePanel();
    previousHandoff.current = observation.handed_over;
  }, [observation.handed_over]);

  /** Release the player's keyboard and pointer as soon as the lock changes state. */
  useEffect(() => {
    if (observation.gate_open || observation.lock_tripped) setPanel(null);
  }, [observation.gate_open, observation.lock_tripped]);

  /** Suspends walking while panels are open or the browser is running in the background. */
  useEffect(() => {
    /** Releases held touch directions and reconciles scene input with browser focus. */
    function syncFocus(event?: Event) {
      clearWalking();
      scene.current?.setWalkingEnabled(walkingEnabled && event?.type !== "blur" && !document.hidden && document.hasFocus());
    }

    /** Preserves a held direction when a second finger starts looking on the canvas. */
    function onFocusIn(event: FocusEvent) {
      if (event.target !== canvas.current) clearWalking();
    }

    syncFocus();
    window.addEventListener("blur", syncFocus);
    window.addEventListener("focus", syncFocus);
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("visibilitychange", syncFocus);
    return () => {
      window.removeEventListener("blur", syncFocus);
      window.removeEventListener("focus", syncFocus);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("visibilitychange", syncFocus);
      clearWalking();
    };
  }, [walkingEnabled]);

  /** Moves focus into any newly opened instrument. */
  useEffect(() => {
    if (activePanel) instrument.current?.focus({ preventScroll: true });
  }, [activePanel]);

  return (
    <main className={`hotel hotel--${observation.wing}`} data-route={observation.route} data-location={observation.exited ? "refuge" : observation.gate_open ? "bridge" : observation.on_landing ? "terrace" : "lift"}>
      <canvas ref={canvas} className="hotel-scene" role="img"
        aria-label={observation.exited ? "Inside the mountain refuge." : observation.gate_open ? "The open gate leads to the refuge across the bridge." : observation.on_landing ? "A hotel terrace overlooking the avalanche. An iron footbridge crosses the ravine to the lit doorway of the refuge." : "An alpine guest room. A bulletin on the cold radiator reports that the avalanche cut the hotel heating main. The mountain patrol has a fire at the refuge. A tall open grille connects this room to a shared air well that carries voices between the lifts. A brass workshop plate hangs beside the lift machinery."}
        data-walkable={walkingEnabled && sceneState === "ready"} />
      <div className="hotel-vignette" aria-hidden="true" />
      <header className="hotel-heading">
        <p className="hotel-caption">Counterweight Hotel</p>
        <h1>{observation.exited ? "Mountain refuge" : wing + (observation.gate_open ? " bridge" : observation.on_landing ? " terrace" : " Room")}</h1>
        <p className="hotel-route-title">{observation.route_title}</p>
        <div className="hotel-view-controls" onKeyDown={event => {
          if (event.key === "Escape") {
            setViewOpen(false);
            viewButton.current?.focus();
          }
        }} onBlur={event => {
          if (!event.currentTarget.contains(event.relatedTarget)) setViewOpen(false);
        }}>
          <button type="button" ref={viewButton} aria-expanded={viewOpen} onClick={() => {
            setViewOpen(open => !open);
            setPanel(null);
          }}>Camera</button>
          {sceneState === "ready" && !observation.exited && <button type="button"
            aria-pressed={revealObjects} onClick={() => {
              scene.current?.setRevealObjects(!revealObjects);
              setRevealObjects(!revealObjects);
            }}>{revealObjects ? "Hide highlights" : "Show interactables"}</button>}
          {viewOpen && <fieldset>
            <legend>Camera settings</legend>
            <label><input type="checkbox" checked={viewSettings.animateLift} onChange={event => setViewSettings({
              ...viewSettings, animateLift: event.target.checked,
            })} /> Animate lift rides</label>
            <label><input type="checkbox" checked={viewSettings.reduced} onChange={event => setViewSettings({
              ...viewSettings, reduced: event.target.checked,
            })} /> Skip guided crossing animation</label>
            <label>Vertical field of view <select value={viewSettings.degrees} onChange={event => setViewSettings({
              ...viewSettings, degrees: Number(event.target.value),
            })}>{viewingAngles.map(angle => <option key={angle} value={angle}>{angle}°</option>)}</select></label>
          </fieldset>}
        </div>
      </header>

      <div className="hotel-indicator" aria-label="Lift position" aria-live="polite">
        <span className="hotel-caption">{traveling ? "In transit" : "Brake set"}</span>
        <strong>{floorName(displayedFloor)}</strong>
        {atRescueBridge && <span className="hotel-arrival">Rescue bridge</span>}
        <span className="hotel-caption">{otherWing} · {floorName(displayedPartnerFloor)}</span>
      </div>

      <div className="hotel-arrival-status" role="status">
        {atRescueBridge && !observation.gate_open && !traveling && (
          <div className="hotel-arrival-banner">
            <strong>Rescue bridges reached</strong>
          </div>
        )}
      </div>

      {sceneState !== "ready" && (
        <p className="hotel-scene-note" role="status">{sceneState === "loading"
          ? "Opening the room…"
          : "The 3D room could not open. The controls are available below."}</p>
      )}

      <div className="hotel-console">
        {walkingEnabled && sceneState === "ready" && (
          <div className="hotel-touch-navigation" role="group" aria-label="Walking controls">
            <div className="hotel-walk-pad">
              {(Object.keys(walkDirections) as WalkDirection[]).map((direction) => (
                <button type="button" key={direction} className={`hotel-walk-pad-${direction}`}
                  aria-label={`Walk ${direction}`}
                  onPointerDown={(event) => holdDirection(event, direction)}
                  onPointerUp={releaseDirection}
                  onPointerCancel={releaseDirection}
                  onLostPointerCapture={releaseDirection}
                  onClick={(event) => { if (event.detail === 0) step(direction); }}>
                  <span aria-hidden="true">{walkDirections[direction].icon}</span>
                </button>
              ))}
            </div>
          </div>
        )}
        {walkingEnabled && sceneState === "ready" && hoverLabel && (
          <span className="hotel-object-label">
            <span className="hotel-outline-sample" aria-hidden="true" />
            <span className="hotel-input-mouse">Click</span><span className="hotel-input-touch">Tap</span>
            {` to ${hoverLabel}`}
          </span>
        )}
        {activePanel && (
          <section ref={instrument} className={`hotel-panel hotel-panel--${activePanel}`} aria-labelledby="panel-title" tabIndex={-1}
            onKeyDown={(event) => { if (event.key === "Escape") closePanel(); }}>
            <div className="hotel-panel-heading">
              <h2 id="panel-title">{activePanel === "switch" ? (observation.wing === "west" ? "West bridge master" : "East brake switch") : panels[activePanel]}</h2>
              <button type="button" className="hotel-close" onClick={closePanel} aria-label="Close panel">×</button>
            </div>
            {(activePanel === "plate" || activePanel === "route-card" || activePanel === "bulletin") && (
              <DocumentArtwork document={activePanel} observation={observation} />
            )}
            {activePanel === "ballast" && (
              <>
                <p>The two lifts hang from opposite ends of one cable. With the brake released, the heavier lift descends and pulls the lighter one up. Only suitcases aboard a lift add to its weight.</p>
                <div className="hotel-loads" aria-live="polite">
                  {[{ name: wing, count: observation.cargo }, { name: otherWing, count: observation.partner_cargo }].map(({ name, count }) => (
                    <div key={name}><strong>{name}</strong><span aria-hidden="true" className="hotel-case-slots">{[0, 1, 2].map(i => <i key={i} data-filled={i < count} />)}</span><small>{count}/3 suitcases</small></div>
                  ))}
                  <p className="hotel-landing-stock">{floorName(observation.floor)} landing rack · {observation.landing_loads[observation.floor]}/6 suitcases</p>
                  <p>{traveling ? "Racks locked during travel" : balance}</p>
                </div>
                {observation.freight_status && observation.freight_floor !== null && <div className="hotel-waybill">
                  <strong>Dispensary waybill</strong>
                  <span>East stores · {floorName(eastStoresFloor(observation.wing, observation.freight_floor))} → Mountain refuge</span>
                  <span>One sealed suitcase · counts toward East’s load</span>
                  <output>{freightReading}</output>
                  {observation.wing === "east" && observation.floor === observation.freight_floor &&
                    observation.freight_status === "awaiting_pickup" &&
                    <>
                      {!can("collect_rescue_case") && <small>{observation.cargo === 3 ? "Car at capacity" : observation.partner_on_landing ? "Terrace interlock engaged" : !observation.handed_over ? "Stores shutter · West circuit open" : "Hoist secured"}</small>}
                      <button type="button" disabled={!can("collect_rescue_case")} onClick={() => act({ type: "collect_rescue_case" })}>Collect dispensary suitcase</button>
                    </>}
                </div>}
                {observation.partner_gate_open && <p>The racks are secured while guests cross.</p>}
                <div className="hotel-panel-actions">
                  <button type="button" disabled={!can("load")} onClick={() => { scene.current?.clearBaggageSelection(); act({ type: "load" }); }}>{observation.load_floor === observation.floor ? "Load one suitcase" : `Load a suitcase from ${floorName(observation.load_floor)}`}</button>
                  <button type="button" disabled={!can("unload")} onClick={() => { scene.current?.clearBaggageSelection(); act({ type: "unload" }); }}>{observation.unload_floor === observation.floor ? "Unload one suitcase" : `Send a suitcase to ${floorName(observation.unload_floor)}`}</button>
                </div>
              </>
            )}
            {activePanel === "brake" && <p>{can("release") ? "The brass key is seated in the switch." : brakeNotice}</p>}
            {(activePanel === "partner" || activePanel === "switch" || activePanel === "key") && <>
              {activePanel === "partner" && <p>{hatchNotice}</p>}
              <p>One brass key serves both brake levers so only one guest can release the shared cable. The East socket unlocks its brake. The West socket also supplies power to both bridge gates.</p>
              {activePanel === "key" && <p>Key seated in the {wingName(observation.handle)} socket.</p>}
              {activePanel === "switch" && <p>{observation.wing === "west"
                ? observation.handed_over ? "Contact closed · bridge lamps live" : "Keyway empty · bridge circuit open"
                : observation.handed_over ? "Keyway empty · brake secured" : "Key seated · brake available"}</p>}
              {can("pass") && <button type="button" onClick={() => act({ type: "pass" })}>Pass brake key to West</button>}
            </>}
            {activePanel === "exit" && <>
              <p>The latch opens for the seal of the inspector who tested this span. The inspection sign in the {otherWing} Room records the seal for this gate.</p>
              <p className="hotel-gate-status">West Room socket circuit · {gatePower} · {gangwayReading}</p>
              <p><b>{observation.wing === "east" ? "East Room" : "West Room"}</b> · {gateReading(observation.gate_open, observation.lock_tripped)}</p>
              <p><b>{observation.wing === "east" ? "West Room" : "East Room"}</b> · {gateReading(observation.partner_gate_open, observation.partner_lock_tripped)}</p>
              {observation.freight_status && <div className="hotel-freight-reading" data-clear={observation.freight_status === "ready"}><b>Freight scale</b> · {freightReading}</div>}
            </>}
            {activePanel === "exit" && (
              <div className="hotel-panel-actions">
                {sealChoices.map((id) => (
                  <button key={id} aria-label={`Press ${seals[id]} seal`} type="button" disabled={!atRescueBridge || !can("exit", id)} onClick={() => act({ type: "exit", seal: id })}>
                    <SealImprint motif={id} />{seals[id]}
                  </button>
                ))}
              </div>
            )}
          </section>
        )}
        {observation.gate_open ? (
          <div className="hotel-terrace-controls">
            {observation.exited ? <><p className="hotel-refuge-wait" role="status">Safe at the refuge · waiting for the {otherWing} guest</p>
              {sceneState === "ready" && <button type="button" onClick={() => scene.current?.watchBridge()}>Look back at bridge</button>}</>
              : <button type="button" disabled={!can("reach_refuge") || sceneState === "loading"} onClick={() => {
                if (atDoor) act({ type: "reach_refuge" });
                else if (scene.current) scene.current.walkToRefuge();
                else if (sceneState === "unavailable") act({ type: "reach_refuge" });
              }}>{atDoor ? "Enter refuge" : "Walk to refuge"}</button>}
          </div>
        ) : observation.on_landing ? (
          <div className="hotel-terrace-controls">
            <button type="button" disabled={!can("board_lift")} onClick={() => act({ type: "board_lift" })}>Return to {wing} Room</button>
          </div>
        ) : <>
        <div className="hotel-cable-gauge" role="status" aria-label="Cable and brake">
          <span>{wing} · {observation.cargo} {observation.cargo === 1 ? "suitcase" : "suitcases"}</span>
          <span>{otherWing} · {observation.partner_cargo} {observation.partner_cargo === 1 ? "suitcase" : "suitcases"}</span>
          <strong>{rewinding ? "Safety spring rewinding…" : observation.lock_tripped ? "Wrong seal · safety catch tripped"  : traveling ? "Lift moving" : observation.partner_gate_open ? "Other gate open · cable secured" : observation.partner_on_landing ? `${otherWing} on terrace · brake locked` : atRescueBridge ? "Docked at rescue bridge" : balance}</strong>
          <span>Brake key · {wingName(observation.handle)}</span>
        </div>
        <div className="hotel-dock" role="group" aria-label="Lift controls">
          <button type="button" disabled={traveling} aria-expanded={activePanel === "ballast"} onClick={(event) => togglePanel("ballast", event.currentTarget)}>Suitcase rack <small>{observation.cargo}/3</small></button>
          <button type="button" disabled={!interactionEnabled || traveling || rewinding} onClick={(event) => {
            if (can("reset_lock")) setRewinding(true);
            else if (can("release")) act({ type: "release" });
            else togglePanel("brake", event.currentTarget);
          }}>{rewinding ? "Rewinding…" : can("reset_lock") ? "Reset safety catch" : can("release") ? "Pull brake lever" : "Inspect brake"}</button>
          <button type="button" disabled={traveling} aria-expanded={activePanel === "partner"} onClick={(event) => togglePanel("partner", event.currentTarget)}>Exchange hatch</button>
          <button type="button" disabled={observation.exited || traveling}
            aria-expanded={activePanel === "exit"} onClick={(event) => togglePanel("exit", event.currentTarget)}>Bridge gate</button>
          <button type="button" disabled={!can("step_onto_landing")} onClick={() => { setPanel(null); act({ type: "step_onto_landing" }); }}>Step outside</button>
          <button type="button" aria-expanded={activePanel === "route-card"} onClick={(event) => togglePanel("route-card", event.currentTarget)}>{otherWing} Room route</button>
          <button type="button" aria-expanded={activePanel === "bulletin"} onClick={(event) => togglePanel("bulletin", event.currentTarget)}>House bulletin</button>
        </div>
        </>}
      </div>
    </main>
  );
}
