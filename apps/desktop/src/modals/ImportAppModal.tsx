import { useState } from 'react';
import { Banner, Button, Input, Modal, Select, Toast } from '@douyinfe/semi-ui';
import { api } from '../api';
import { copy } from '../copy';
import type { Brand } from '../types';
import { normalizeError } from '../utils';

export interface ImportAppModalProps {
  visible: boolean;
  onClose: () => void;
  onDone: () => Promise<void>;
}

export function ImportAppModal(props: ImportAppModalProps) {
  const [label, setLabel] = useState('');
  const [appId, setAppId] = useState('');
  const [secret, setSecret] = useState('');
  const [brand, setBrand] = useState<Brand>('feishu');
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    try {
      setBusy(true);
      await api.importExistingApp({ label: label.trim(), appId: appId.trim(), appSecret: secret, brand });
      setSecret('');
      Toast.success(copy.importApp.done);
      props.onClose();
      await props.onDone();
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={copy.importApp.title}
      visible={props.visible}
      onCancel={props.onClose}
      className="lpc-modal"
      modalContentClass="lpc-modal-content"
      footer={(
        <>
          <Button disabled={busy} onClick={props.onClose}>{copy.common.cancel}</Button>
          <Button
            theme="solid"
            loading={busy}
            disabled={!label.trim() || !appId.trim() || !secret}
            onClick={() => void submit()}
          >
            {copy.importApp.submit}
          </Button>
        </>
      )}
      width={540}
    >
      <div className="modal-stack">
        <Banner
          type="warning"
          description={copy.importApp.banner}
        />
        <div className="form-stack">
          <label>{copy.importApp.label}<Input value={label} onChange={setLabel} placeholder={copy.importApp.labelPlaceholder} /></label>
          <label>{copy.importApp.appId}<Input value={appId} onChange={setAppId} placeholder={copy.importApp.appIdPlaceholder} /></label>
          <label>{copy.importApp.secret}<Input mode="password" value={secret} onChange={setSecret} autoComplete="off" /></label>
          <label>
            <span id="import-app-brand-label">{copy.importApp.brand}</span>
            <Select
              aria-labelledby="import-app-brand-label"
              value={brand}
              onChange={(value) => setBrand(String(value) as Brand)}
              inputProps={{ 'aria-label': '导入 App 品牌' }}
              optionList={[
                { value: 'feishu', label: copy.importApp.feishu },
                { value: 'lark', label: copy.importApp.lark },
              ]}
            />
          </label>
        </div>
      </div>
    </Modal>
  );
}
