import { eastStoresFloor, type Seal } from "./game";

/** Pick targets for the workshop plate, bridge inspection sign and house bulletin. */
export type HotelDocument = "plate" | "route-card" | "bulletin";

/** Private rescue route details present in the local guest's observation. */
export interface Booking {
  room: "warm" | "cool";
  partnerDestination?: number;
  partnerSeal?: Seal;
  partnerDelivery?: number | null;
  freightFloor?: number | null;
}

export const documentWidth = 960;
export const documentHeight = 640;

/** Gives the room textures a stable revision key when unrelated scene state changes. */
export function bookingRevision(booking: Booking): string {
  return [booking.room, booking.partnerDestination, booking.partnerSeal, booking.partnerDelivery, booking.freightFloor].join("/");
}

/** Names the landing as it appears on the inspection sign. */
function bookingFloor(booking: Booking): string {
  const destination = booking.partnerDestination;
  if (destination == null) return "—";
  return destination === 0 ? "Ground" : String(destination);
}

/** Names the wax imprint so it can be read without identifying the illustration. */
function bookingSeal(booking: Booking): string {
  const motif = booking.partnerSeal;
  return motif ? motif[0].toUpperCase() + motif.slice(1) : "—";
}

type Ink = CanvasRenderingContext2D;

/** Strokes one continuous engraved contour using the current pen. */
function line(c: Ink, ...points: number[]) {
  c.beginPath();
  c.moveTo(points[0], points[1]);
  for (let i = 2; i < points.length; i += 2) c.lineTo(points[i], points[i + 1]);
  c.stroke();
}

/** Draws a sheave with a central bearing and six cast spokes. */
function sheave(c: Ink, x: number, y: number, radius: number) {
  for (const r of [radius, radius * 0.76, radius * 0.15]) {
    c.beginPath(); c.arc(x, y, r, 0, Math.PI * 2); c.stroke();
  }
  for (let i = 0; i < 6; i++) {
    const a = i * Math.PI / 3;
    line(c, x + 4 * Math.cos(a), y + 4 * Math.sin(a),
      x + radius * 0.76 * Math.cos(a), y + radius * 0.76 * Math.sin(a));
  }
}

/** Prints a strapped trunk with its handle so case counts read as ballast. */
function trunk(c: Ink, x: number, y: number) {
  c.strokeRect(x, y, 38, 23);
  line(c, x + 8, y, x + 8, y + 23);
  line(c, x + 30, y, x + 30, y + 23);
  line(c, x + 14, y, x + 14, y - 4, x + 24, y - 4, x + 24, y);
}

/** Draws the riveted car in section and stacks its cargo against the sill. */
function cabin(c: Ink, x: number, y: number, cases: number) {
  line(c, x - 48, y + 8, x, y - 12, x + 48, y + 8);
  c.strokeRect(x - 48, y + 8, 96, 86);
  c.strokeRect(x - 51, y + 94, 102, 7);
  for (let i = 0; i < cases; i++)
    trunk(c, x - 41 + (i % 2) * 43, y + 67 - Math.floor(i / 2) * 29);
}

/** Reproduces the commissioning plate's cutaway and its separate exchange detail at stop II. */
function engineering(c: Ink) {
  c.fillStyle = "#234744"; c.fillRect(0, 0, 960, 640);
  c.strokeStyle = "#c3b58b"; c.fillStyle = "#ecdbad"; c.lineWidth = 2;
  c.strokeRect(18, 18, 924, 604); c.strokeRect(25, 25, 910, 590);
  c.textAlign = "center"; c.font = "36px Georgia"; c.fillText("ATELIERS DU FUNICULAIRE", 480, 77);
  c.font = "18px Georgia"; c.fillText("M O N T A G N E     •     1 8 9 6", 480, 111);
  line(c, 54, 133, 906, 133); line(c, 552, 149, 552, 559);
  c.lineWidth = 3;
  for (const x of [193, 408]) sheave(c, x, 216, 32);
  line(c, 161, 216, 161, 346); line(c, 193, 184, 408, 184);
  line(c, 440, 216, 440, 251);
  cabin(c, 161, 358, 3); cabin(c, 440, 263, 1);
  line(c, 249, 316, 249, 434, 237, 417); line(c, 249, 434, 261, 417);
  line(c, 352, 376, 352, 258, 340, 275); line(c, 352, 258, 364, 275);
  c.font = "28px Georgia"; c.fillText("Suitcases add weight.", 293, 504);
  c.fillText("The heavier car descends.", 293, 539);
  c.font = "26px Georgia"; c.fillText("Hatches meet at Floor II", 743, 212);
  cabin(c, 635, 307, 0); cabin(c, 847, 307, 0);
  line(c, 635, 243, 635, 295); line(c, 847, 243, 847, 295);
  c.strokeStyle = "#f2c477"; c.fillStyle = "#f2c477"; c.lineWidth = 9;
  c.beginPath(); c.arc(718, 324, 13, 0, Math.PI * 2); c.stroke();
  line(c, 731, 324, 775, 324); line(c, 760, 324, 760, 335); line(c, 772, 324, 772, 341);
  c.lineWidth = 3; line(c, 690, 370, 792, 370, 781, 361); line(c, 792, 370, 781, 379);
  c.fillStyle = "#ecdbad"; c.font = "24px Georgia"; c.fillText("Both bridge gate releases", 741, 539);
  line(c, 54, 557, 906, 557);
  // The West contact feeds a pilot lamp in each wing. Its engraved circuit repeats the marks on the real fittings.
  c.font = "25px Georgia"; c.fillText("WEST MASTER", 741, 448);
  c.lineWidth = 2;
  line(c, 650, 477, 680, 477, 711, 456);
  line(c, 717, 477, 845, 477);
  for (const x of [760, 842]) {
    line(c, x, 477, x, 487);
    c.beginPath(); c.arc(x, 497, 10, 0, Math.PI * 2); c.stroke();
    line(c, x - 6, 491, x + 6, 503); line(c, x + 6, 491, x - 6, 503);
  }
  c.font = "27px Georgia"; c.fillText("One brake key serves both cars. In West it also feeds both gates.", 480, 596);
}

/** Engraves a wax seal using the same three motifs named by the route sign and door. */
export function drawSealMark(c: Ink, motif: Seal) {
  c.fillStyle = "#873f35"; c.strokeStyle = "#edbc8b"; c.lineWidth = 3;
  c.beginPath(); c.arc(0, 0, 44, 0, Math.PI * 2); c.fill();
  c.beginPath(); c.arc(0, 0, 38, 0, Math.PI * 2); c.stroke();
  if (motif === "stag") {
    line(c, -12,-6,-8,13,0,22,8,13,12,-6,0,-2,-12,-6);
    for (const d of [-1,1]) {
      line(c, d*8,-4,d*20,-14,d*24,-27);
      line(c, d*16,-11,d*12,-24); line(c,d*21,-18,d*29,-20);
    }
  } else if (motif === "key") {
    c.beginPath(); c.arc(-10,-10,10,0,Math.PI*2); c.stroke();
    line(c,-3,-3,20,20,26,14); line(c,12,12,18,6);
  } else {
    for (let i = 0; i < 5; i++) {
      c.save(); c.rotate(i * Math.PI * 2 / 5);
      c.beginPath(); c.ellipse(0,-10,9,14,0,0,Math.PI*2); c.stroke(); c.restore();
    }
  }
}

/** Identifies the partner lift's rescue bridge on the wall card. */
function booking(c: Ink, s: Booking) {
  const other = s.room === "warm" ? "WEST ROOM" : "EAST ROOM";
  c.fillStyle = "#dfcea7"; c.fillRect(0,0,960,640);
  c.fillStyle = s.room === "warm" ? "#335b60" : "#864739"; c.fillRect(31,31,898,153);
  c.textAlign = "center"; c.fillStyle = "#ffedce"; c.font = "45px Georgia";
  c.fillText("Hôtel du Contrepoids",480,101); c.font = "23px Georgia";
  c.fillText("R E S C U E   B R I D G E   R O U T E",480,149);
  c.fillStyle = "#48372e"; c.strokeStyle = "#887658"; c.lineWidth = 2;
  c.strokeRect(31,184,898,422); line(c,65,273,895,273); line(c,65,489,895,489);
  c.font = "bold 48px Georgia"; c.fillText(other,480,235);
  c.font = "italic 25px Georgia"; c.fillText("For the guest across the shaft",480,264);
  c.font = "23px Georgia"; c.fillText("BRIDGE FLOOR",295,320); c.fillText("INSPECTOR’S SEAL",655,320);
  c.font = s.partnerDestination === 0 ? "65px Georgia" : "103px Georgia";
  c.fillText(bookingFloor(s),295,439);
  if (s.partnerSeal) {
    c.save(); c.translate(655,401); c.scale(1.4,1.4); drawSealMark(c,s.partnerSeal); c.restore();
    c.fillStyle = "#48372e"; c.font = "24px Georgia"; c.fillText(bookingSeal(s),655,475);
  }
  if (s.partnerDelivery == null) {
    c.font = "italic 29px Georgia"; c.fillText("Span tested and cleared",480,559);
  } else {
    trunk(c,78,510);
    c.font = "30px Georgia"; c.fillText(other + " cargo · " + s.partnerDelivery + (s.partnerDelivery === 1 ? " suitcase" : " suitcases"),515,538);
    if (s.freightFloor != null) {
      const floor = eastStoresFloor(s.room === "warm" ? "east" : "west", s.freightFloor);
      c.font = "22px Georgia"; c.fillText("East Room cargo includes the dispensary suitcase from East stores, Floor " + floor + ".",480,582);
    }
  }
}

/** The concierge's emergency bulletin is the same print clipped to each cold radiator. */
function bulletin(c: Ink) {
  c.fillStyle = "#f5e6c8"; c.fillRect(0, 0, 960, 640);
  c.strokeStyle = "#a18c65"; c.lineWidth = 3; c.strokeRect(24, 24, 912, 592);
  c.textAlign = "center";
  c.fillStyle = "#584432"; c.font = "28px Georgia";
  c.fillText("HÔTEL DU CONTREPOIDS     •     NIGHT DESK", 480, 87);
  c.fillStyle = "#963f36"; c.fillRect(25, 124, 910, 119);
  c.fillStyle = "#fff3d3"; c.font = "bold 66px Georgia"; c.fillText("HEATING OUT", 480, 204);
  c.fillStyle = "#342b25"; c.font = "37px Georgia";
  c.fillText("The avalanche tore out the steam main", 480, 327);
  c.fillText("and buried the valley stairs.", 480, 380);
  c.font = "italic 35px Georgia";
  c.fillText("The patrol has a fire and beds", 480, 490);
  c.fillText("at the mountain refuge.", 480, 541);
}

/** Paints the complete document onto a 960 × 640 canvas in world or in close-up. */
export function drawHotelDocument(context: CanvasRenderingContext2D, document: HotelDocument, entry: Booking) {
  switch (document) {
    case "plate": engineering(context); break;
    case "route-card": booking(context, entry); break;
    case "bulletin": bulletin(context); break;
  }
}
