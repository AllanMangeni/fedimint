# Setting Up Fedimint Gateway on Start9

## Lightning Backend

Choose between two Lightning implementations:

### LDK (Integrated) - Recommended for new users
- No additional setup required
- The Gateway runs its own integrated Lightning node
- Simply configure and start the service

### LND (External) - For existing LND users
1. Install and configure LND from the Start9 marketplace
2. Ensure LND is fully synced before starting the Gateway
3. Select "LND" as the Lightning backend in Gateway config
4. The Gateway will automatically connect to your LND node

## Setup Steps

1. **Choose Lightning Backend**: Select either LDK (integrated) or LND (external)
2. **Configure Bitcoin Backend**: Use your Bitcoin node (recommended) or Esplora API
3. **Set Admin Password**: Enter a strong password (minimum 8 characters)
4. **Start the Service**: Launch from your Start9 dashboard
5. **Wait for Health Check**: The web interface will indicate when the Gateway is ready
6. **Access the Dashboard**: Open the Gateway UI and log in with your password
7. **Join Federations**: Enter federation invite codes to connect

## Notes

- If using LND, ensure it is running and synced before starting the Gateway
- The Gateway stores its data separately from LND, so both can be backed up independently
- Changing from LDK to LND (or vice versa) requires reconfiguring the Gateway

## Learn More

Visit [fedimint.org](https://fedimint.org) for comprehensive documentation about the Fedimint protocol, gateway concepts, and setup guidance.
