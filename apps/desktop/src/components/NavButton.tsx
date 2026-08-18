import type { ReactNode } from 'react';

export interface NavButtonProps {
  active: boolean;
  icon: ReactNode;
  children: ReactNode;
  onClick: () => void;
}

export function NavButton(props: NavButtonProps) {
  return (
    <button className={`nav-button ${props.active ? 'active' : ''}`} onClick={props.onClick}>
      {props.icon}
      <span>{props.children}</span>
    </button>
  );
}
