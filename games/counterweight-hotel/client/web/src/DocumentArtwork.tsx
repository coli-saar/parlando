import { useLayoutEffect, useRef } from "react";
import { eastStoresFloor, type Observation, type Seal } from "./game";
import { documentHeight, documentWidth, drawHotelDocument, drawSealMark } from "./document-art";
import type { HotelDocument } from "./document-art";
import "./document-art.css";

interface Props {
  document: HotelDocument;
  observation: Pick<Observation, "wing" | "freight_floor" | "partner_destination" | "partner_seal" | "partner_delivery">;
}

/** Magnifies the same printed face used on the room model and provides its information as accessible text. */
export function DocumentArtwork({ document, observation }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const { wing, freight_floor, partner_destination, partner_seal, partner_delivery } = observation;
  const otherWing = wing === "east" ? "West" : "East";
  const floor = partner_destination === 0 ? "Ground" : "floor " + partner_destination;
  const delivery = partner_delivery == null ? "" :
    " The " + otherWing + " Room cargo is " + partner_delivery + (partner_delivery === 1 ? " suitcase." : " suitcases.");
  const description = document === "bulletin"
    ? "Heating out. The avalanche tore out the steam main and buried the valley stairs. The patrol has a fire and beds at the mountain refuge."
    : document === "plate"
    ? "Ateliers du Funiculaire, 1896. One cable connects two cars. The car carrying more suitcases descends as the lighter car rises. At the mark II, the aligned hatches allow the single brake key to pass between cars. It operates the brake in either car. In West it also supplies the gate releases at both bridges."
    : `Rescue bridge for the ${otherWing} Room, across the shaft from you. Bridge floor: ${floor}. Gate seal: ${partner_seal}.` + delivery + (partner_delivery != null && freight_floor != null ? ` Dispensary suitcase at East stores, Floor ${eastStoresFloor(wing, freight_floor)}, included in East’s consignment.` : "");

  useLayoutEffect(() => {
    const context = canvas.current?.getContext("2d");
    if (!context) return;
    drawHotelDocument(context, document, {
      room: wing === "east" ? "warm" : "cool",
      partnerDestination: partner_destination,
      partnerSeal: partner_seal,
      partnerDelivery: partner_delivery,
      freightFloor: freight_floor,
    });
  }, [document, wing, freight_floor, partner_destination, partner_seal, partner_delivery]);

  return <canvas className="hotel-document-art" ref={canvas} width={documentWidth} height={documentHeight}
    role="img" aria-label={description} />;
}

/** Shows the inspection stamp engraved on a gate release, using the route sign’s inspection mark. */
export function SealImprint({ motif }: { motif: Seal }) {
  const face = useRef<HTMLCanvasElement>(null);
  useLayoutEffect(() => {
    const ink = face.current?.getContext("2d");
    if (!ink) return;
    ink.clearRect(0, 0, 96, 96);
    ink.save();
    ink.translate(48, 48);
    drawSealMark(ink, motif);
    ink.restore();
  }, [motif]);
  return <canvas ref={face} width={96} height={96} aria-hidden="true" />;
}
