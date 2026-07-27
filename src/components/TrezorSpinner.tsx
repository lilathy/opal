/** Official rotating-device clips from Trezor's own asset set (trezor-suite), used as-is. */
import modelOne from "../assets/trezor/t1b1.webm";
import modelT from "../assets/trezor/t2t1.webm";
import safe3 from "../assets/trezor/safe3.webm";
import safe5 from "../assets/trezor/safe5.webm";
import safe7 from "../assets/trezor/safe7.webm";

const VIDEO_BY_INTERNAL_MODEL: Record<string, string> = {
  T1B1: modelOne,
  T2T1: modelT,
  T2B1: safe3,
  T3B1: safe3,
  T3T1: safe5,
  T3W1: safe7,
};

type Props = {
  internalModel?: string | null;
  size?: number;
  className?: string;
};

export function TrezorSpinner({ internalModel, size = 30, className }: Props) {
  const src = (internalModel && VIDEO_BY_INTERNAL_MODEL[internalModel.toUpperCase()]) || safe3;
  const reduceMotion =
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  return (
    <video
      key={src}
      className={className}
      style={{ width: size, height: size, objectFit: "contain" }}
      src={src}
      autoPlay={!reduceMotion}
      loop={!reduceMotion}
      muted
      playsInline
      disablePictureInPicture
    />
  );
}
