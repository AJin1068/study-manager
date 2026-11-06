import { svgDataUri } from '../pages/characterView';

/** The character picture. The svg comes from the core (placeholder shapes until real art lands). */
export function CharacterAvatar(props: { svg: string; label: string; size?: number }) {
  const size = props.size ?? 96;
  return <img src={svgDataUri(props.svg)} alt={props.label} width={size} height={size} className="character-avatar" />;
}
